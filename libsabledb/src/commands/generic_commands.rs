use crate::commands::ErrorStrings;
#[allow(unused_imports)]
use crate::{
    check_args_count, check_value_type,
    client::ClientState,
    command_arg_at,
    commands::{HandleCommandResult, StringCommands},
    metadata::CommonValueMetadata,
    metadata::Encoding,
    parse_string_to_number,
    storage::GenericDb,
    types::List,
    BytesMutUtils, Expiration, LockManager, PrimaryKeyMetadata, RedisCommand, RedisCommandName,
    RespBuilderV2, SableError, StorageAdapter, StringUtils, Telemetry, TimeUtils,
};

use bytes::BytesMut;
use std::rc::Rc;
use tokio::io::AsyncWriteExt;

pub struct GenericCommands {}

impl GenericCommands {
    pub async fn handle_command(
        client_state: Rc<ClientState>,
        command: Rc<RedisCommand>,
        _tx: &mut (impl AsyncWriteExt + std::marker::Unpin),
    ) -> Result<HandleCommandResult, SableError> {
        let mut response_buffer = BytesMut::with_capacity(256);
        match command.metadata().name() {
            RedisCommandName::Ttl => {
                Self::ttl(client_state, command, &mut response_buffer).await?;
            }
            RedisCommandName::Del => {
                Self::del(client_state, command, &mut response_buffer).await?;
            }
            RedisCommandName::Exists => {
                Self::exists(client_state, command, &mut response_buffer).await?;
            }
            RedisCommandName::Expire => {
                Self::expire(client_state, command, &mut response_buffer).await?;
            }
            _ => {
                return Err(SableError::InvalidArgument(format!(
                    "Non generic command {}",
                    command.main_command()
                )));
            }
        }
        Ok(HandleCommandResult::ResponseBufferUpdated(response_buffer))
    }

    /// O(N) where N is the number of keys that will be removed. When a key to remove holds a value other than a string,
    /// the individual complexity for this key is O(M) where M is the number of elements in the list, set, sorted
    /// set or hash. Removing a single key that holds a string value is O(1).
    async fn del(
        client_state: Rc<ClientState>,
        command: Rc<RedisCommand>,
        response_buffer: &mut BytesMut,
    ) -> Result<(), SableError> {
        check_args_count!(command, 2, response_buffer);

        let mut iter = command.args_vec().iter();
        let _ = iter.next(); // skip the first param which is the command name

        let mut deleted_items = 0usize;
        let db_id = client_state.database_id();
        for user_key in iter {
            // obtain the lock per key
            let _unused = LockManager::lock_user_key_exclusive(user_key, db_id);
            let key_type =
                Self::query_key_type(client_state.clone(), command.clone(), user_key).await?;
            match key_type {
                Some(Encoding::VALUE_STRING) => {
                    let generic_db = GenericDb::with_storage(client_state.database(), db_id);
                    generic_db.delete(user_key)?;
                    deleted_items = deleted_items.saturating_add(1);
                }
                Some(Encoding::VALUE_LIST) => {
                    let list = List::with_storage(client_state.database(), db_id);
                    list.remove(
                        user_key,
                        None, // remove all items
                        i32::MAX,
                        response_buffer,
                    )?;
                    deleted_items = deleted_items.saturating_add(1);
                }
                Some(unknown_type) => {
                    tracing::warn!(
                        "Deleting unknown type found in database for key `{:?}`. type=`{}`",
                        user_key,
                        unknown_type
                    );
                    let generic_db = GenericDb::with_storage(client_state.database(), db_id);
                    generic_db.delete(user_key)?;
                    deleted_items = deleted_items.saturating_add(1);
                }
                _ => {}
            }
        }

        let builder = RespBuilderV2::default();
        builder.number::<usize>(response_buffer, deleted_items, false);
        Ok(())
    }

    /// Returns the remaining time to live of a key that has a timeout.
    /// This introspection capability allows a Redis client to check how
    /// many seconds a given key will continue to be part of the dataset.
    async fn ttl(
        client_state: Rc<ClientState>,
        command: Rc<RedisCommand>,
        response_buffer: &mut BytesMut,
    ) -> Result<(), SableError> {
        check_args_count!(command, 2, response_buffer);
        let builder = RespBuilderV2::default();
        let key = command_arg_at!(command, 1);

        let _unused = LockManager::lock_user_key_shared(key, client_state.database_id());
        let generic_db =
            GenericDb::with_storage(client_state.database(), client_state.database_id());
        if let Some((_, value_metadata)) = generic_db.get(key)? {
            if !value_metadata.expiration().has_ttl() {
                // No timeout
                builder.number_i64(response_buffer, -1);
            } else {
                builder.number_u64(
                    response_buffer,
                    value_metadata.expiration().ttl_in_seconds()?,
                );
            }
        } else {
            // The command returns -2 if the key does not exist.
            builder.number_i64(response_buffer, -2);
        }
        Ok(())
    }

    /// Returns if key exists.
    /// The user should be aware that if the same existing key is mentioned in the arguments multiple times,
    /// it will be counted multiple times. So if somekey exists, EXISTS somekey somekey will return 2.
    async fn exists(
        client_state: Rc<ClientState>,
        command: Rc<RedisCommand>,
        response_buffer: &mut BytesMut,
    ) -> Result<(), SableError> {
        // at least 2 items: EXISTS <KEY1> [..]
        check_args_count!(command, 2, response_buffer);

        let mut iter = command.args_vec().iter();
        let _ = iter.next(); // skip the first param which is the command name
        let mut items_found = 0usize;
        let db_id = client_state.database_id();

        let generic_db = GenericDb::with_storage(client_state.database(), db_id);
        for user_key in iter {
            if generic_db.contains(user_key)? {
                items_found = items_found.saturating_add(1);
            }
        }

        let builder = RespBuilderV2::default();
        builder.number_usize(response_buffer, items_found);
        Ok(())
    }

    /// Load entry from the database, don't care about the value type
    async fn query_key_type(
        client_state: Rc<ClientState>,
        _command: Rc<RedisCommand>,
        user_key: &BytesMut,
    ) -> Result<Option<u8>, SableError> {
        let generic_db =
            GenericDb::with_storage(client_state.database(), client_state.database_id());
        let Some((_, md)) = generic_db.get(user_key)? else {
            return Ok(None);
        };
        Ok(Some(md.value_type()))
    }

    async fn expire(
        client_state: Rc<ClientState>,
        command: Rc<RedisCommand>,
        response_buffer: &mut BytesMut,
    ) -> Result<(), SableError> {
        // at least 3 arguments
        check_args_count!(command, 3, response_buffer);
        let builder = RespBuilderV2::default();

        // EXPIRE key seconds [NX | XX | GT | LT]
        let key = command_arg_at!(command, 1);
        let seconds = command_arg_at!(command, 2);

        // Convert into seconds
        let Some(seconds) = BytesMutUtils::parse::<u64>(seconds) else {
            builder.error_string(
                response_buffer,
                ErrorStrings::VALUE_NOT_AN_INT_OR_OUT_OF_RANGE,
            );
            return Ok(());
        };

        let db_id = client_state.database_id();
        let _unused = LockManager::lock_user_key_exclusive(key, db_id);
        let generic_db = GenericDb::with_storage(client_state.database(), db_id);

        // Make sure the key exists in the database
        let Some(mut expiration) = generic_db.get_expiration(key)? else {
            builder.number_usize(response_buffer, 0);
            return Ok(());
        };

        // If no other param was provided, set the ttl in seconds and leave
        let Some(arg) = command.arg(3) else {
            expiration.set_ttl_seconds(seconds)?;
            generic_db.put_expiration(key, &expiration)?;
            builder.number_usize(response_buffer, 1);
            return Ok(());
        };

        let arg = BytesMutUtils::to_string(arg).to_lowercase();
        match arg.as_str() {
            "nx" => {
                // NX -- Set expiry only when the key has no expiry
                if !expiration.has_ttl() {
                    expiration.set_ttl_seconds(seconds)?;
                    generic_db.put_expiration(key, &expiration)?;
                    builder.number_usize(response_buffer, 1);
                } else {
                    builder.number_usize(response_buffer, 0);
                }
            }
            "xx" => {
                // XX -- Set expiry only when the key has an existing expiry
                if expiration.has_ttl() {
                    expiration.set_ttl_seconds(seconds)?;
                    generic_db.put_expiration(key, &expiration)?;
                    builder.number_usize(response_buffer, 1);
                } else {
                    builder.number_usize(response_buffer, 0);
                }
            }
            "gt" => {
                // GT -- Set expiry only when the new expiry is greater than current one
                if seconds > expiration.ttl_in_seconds()? {
                    expiration.set_ttl_seconds(seconds)?;
                    generic_db.put_expiration(key, &expiration)?;
                    builder.number_usize(response_buffer, 1);
                } else {
                    builder.number_usize(response_buffer, 0);
                }
            }
            "lt" => {
                // LT -- Set expiry only when the new expiry is less than current one
                if seconds < expiration.ttl_in_seconds()? {
                    expiration.set_ttl_seconds(seconds)?;
                    generic_db.put_expiration(key, &expiration)?;
                    builder.number_usize(response_buffer, 1);
                } else {
                    builder.number_usize(response_buffer, 0);
                }
            }
            option => {
                builder.error_string(
                    response_buffer,
                    format!("ERR Unsupported option {}", option).as_str(),
                );
            }
        }
        Ok(())
    }
}

//  _    _ _   _ _____ _______      _______ ______  _____ _______ _____ _   _  _____
// | |  | | \ | |_   _|__   __|    |__   __|  ____|/ ____|__   __|_   _| \ | |/ ____|
// | |  | |  \| | | |    | |    _     | |  | |__  | (___    | |    | | |  \| | |  __|
// | |  | | . ` | | |    | |   / \    | |  |  __|  \___ \   | |    | | | . ` | | |_ |
// | |__| | |\  |_| |_   | |   \_/    | |  | |____ ____) |  | |   _| |_| |\  | |__| |
//  \____/|_| \_|_____|  |_|          |_|  |______|_____/   |_|  |_____|_| \_|\_____|
//
#[cfg(test)]
mod test {
    use super::*;
    use crate::{commands::ClientNextAction, Client, ServerState};
    use std::rc::Rc;
    use std::sync::Arc;
    use test_case::test_case;

    #[test_case(vec![
        (vec!["set", "mystr", "myvalue"], "+OK\r\n"),
        (vec!["set", "mystr2", "myvalue2"], "+OK\r\n"),
        (vec!["lpush", "mylist_1", "a", "b", "c"], ":3\r\n"),
        (vec!["lpush", "mylist_2", "a", "b", "c"], ":3\r\n"),
        (vec!["del", "mystr", "mystr2", "mylist_1", "mylist_2"], ":4\r\n"),
        (vec!["get", "mystr"], "$-1\r\n"),
        (vec!["get", "mystr2"], "$-1\r\n"),
        (vec!["llen", "mylist_1"], ":0\r\n"),
        (vec!["llen", "mylist_2"], ":0\r\n"),
        (vec!["del", "mylist_2"], ":0\r\n"),
    ], "test_del"; "test_del")]
    #[test_case(vec![
        (vec!["set", "mykey1", "myvalue"], "+OK\r\n"),
        (vec!["set", "mykey2", "myvalue1"], "+OK\r\n"),
        (vec!["exists", "mykey1", "mykey2"], ":2\r\n"),
        (vec!["exists", "mykey1", "mykey2", "mykey1"], ":3\r\n"),
        (vec!["exists", "no_such_key", "mykey2", "mykey1"], ":2\r\n"),
    ], "test_exists"; "test_exists")]
    #[test_case(vec![
        (vec!["set", "mykey1", "myvalue"], "+OK\r\n"),
        (vec!["expire", "mykey1", "100"], ":1\r\n"),
        (vec!["get", "mykey1"], "$7\r\nmyvalue\r\n"),
        (vec!["set", "mykey2", "myvalue", "EX", "100"], "+OK\r\n"),
        (vec!["expire", "mykey2", "90", "GT"], ":0\r\n"),
        (vec!["expire", "mykey2", "120", "GT"], ":1\r\n"),
        (vec!["get", "mykey2"], "$7\r\nmyvalue\r\n"),
        (vec!["set", "mykey3", "myvalue", "EX", "100"], "+OK\r\n"),
        (vec!["expire", "mykey3", "123", "LT"], ":0\r\n"),
        (vec!["expire", "mykey3", "90", "LT"], ":1\r\n"),
        (vec!["get", "mykey3"], "$7\r\nmyvalue\r\n"),
        (vec!["set", "mykey4", "myvalue", "EX", "100"], "+OK\r\n"),
        (vec!["expire", "mykey4", "120", "NX"], ":0\r\n"),
        (vec!["expire", "mykey4", "120", "XX"], ":1\r\n"),
        (vec!["set", "mykey5", "myvalue"], "+OK\r\n"),
        (vec!["expire", "mykey5", "120", "XX"], ":0\r\n"),
        (vec!["expire", "mykey5", "120", "NX"], ":1\r\n"),
    ], "test_expire"; "test_expire")]
    fn test_generic_commands(
        args_vec: Vec<(Vec<&'static str>, &'static str)>,
        test_name: &str,
    ) -> Result<(), SableError> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let (_guard, store) = crate::tests::open_store();
            let client = Client::new(Arc::<ServerState>::default(), store, None);

            for (args, expected_value) in args_vec {
                let mut sink = crate::tests::ResponseSink::with_name(test_name).await;
                let cmd = Rc::new(RedisCommand::for_test(args));
                match Client::handle_command(client.inner(), cmd, &mut sink.fp)
                    .await
                    .unwrap()
                {
                    ClientNextAction::NoAction => {
                        assert_eq!(sink.read_all().await.as_str(), expected_value);
                    }
                    _ => {}
                }
            }
        });
        Ok(())
    }
}
