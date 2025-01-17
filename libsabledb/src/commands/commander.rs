use bytes::BytesMut;
use std::collections::HashMap;
use strum_macros::EnumString;

#[derive(Default, Debug, Clone, EnumString)]
pub enum RedisCommandFlags {
    #[default]
    None = 0,
    /// A read command
    #[strum(serialize = "read")]
    Read = 1 << 0,
    /// A write command
    #[strum(serialize = "write")]
    Write = 1 << 1,
    /// Administration command
    #[strum(serialize = "admin")]
    Admin = 1 << 2,
    /// @connection command
    #[strum(serialize = "connection")]
    Connection = 1 << 3,
    /// Command might block the client
    #[strum(serialize = "blocking")]
    Blocking = 1 << 4,
}

#[derive(Clone, Debug, Default, EnumString)]
pub enum RedisCommandName {
    Append,
    Decr,
    DecrBy,
    IncrBy,
    IncrByFloat,
    Incr,
    #[default]
    Set,
    Get,
    Mget,
    Mset,
    Msetnx,
    GetDel,
    GetSet,
    GetEx,
    GetRange,
    Lcs,
    Ping,
    Config,
    Psetex,
    Setex,
    Setnx,
    SetRange,
    Strlen,
    Substr,
    // List commands
    Lpush,
    Lpushx,
    Rpush,
    Rpushx,
    Lpop,
    Rpop,
    Llen,
    Lindex,
    Linsert,
    Lset,
    Lpos,
    Ltrim,
    Lrange,
    Lrem,
    Lmove,
    Rpoplpush,
    Brpoplpush,
    Blpop,
    Brpop,
    Lmpop,
    Blmpop,
    Blmove,
    // Client commands
    Client,
    Select,
    // Server commands
    ReplicaOf,
    SlaveOf,
    Info,
    Command,
    // Generic commands
    Ttl,
    Del,
    Exists,
    Expire,
    // Hash commands
    Hset,
    Hget,
    Hdel,
    Hlen,
    Hexists,
    Hgetall,
    Hincrby,
    Hincrbyfloat,
    Hkeys,
    Hvals,
    Hmget,
    Hmset,
    Hrandfield,
    NotSupported(String),
}

pub struct CommandsManager {
    cmds: HashMap<&'static str, CommandMetadata>,
}

impl CommandsManager {
    /// Return the metadata for a command
    pub fn metadata(&self, cmdname: &str) -> CommandMetadata {
        match self.cmds.get(cmdname) {
            Some(t) => t.clone(),
            None => CommandMetadata::new(RedisCommandName::NotSupported(format!(
                "unsupported command {}",
                cmdname
            ))),
        }
    }

    /// Return the entire command table into RESPv2 response
    pub fn cmmand_output(&self) -> BytesMut {
        let builder = crate::RespBuilderV2::default();
        let mut buffer = BytesMut::with_capacity(4096);

        builder.add_array_len(&mut buffer, self.cmds.len());
        for cmd_md in self.cmds.values() {
            builder.add_resp_string(&mut buffer, &cmd_md.to_resp_v2());
        }
        buffer
    }

    /// Return the entire command table into RESPv2 response
    pub fn cmmand_docs_output(&self) -> BytesMut {
        let builder = crate::RespBuilderV2::default();
        let mut buffer = BytesMut::with_capacity(4096);

        builder.add_array_len(&mut buffer, self.cmds.len() * 2);
        for name in self.cmds.keys() {
            builder.add_bulk_string_u8_arr(&mut buffer, name.as_bytes());
            builder.add_empty_array(&mut buffer);
        }
        buffer
    }

    /// Return the commands table
    pub fn all_commands(&self) -> &HashMap<&'static str, CommandMetadata> {
        &self.cmds
    }
}

#[derive(Default, Debug, Clone)]
pub struct CommandMetadata {
    cmd_name: RedisCommandName,
    cmd_flags: u64,
    /// Arity is the number of arguments a command expects. It follows a simple pattern:
    /// A positive integer means a fixed number of arguments.
    /// A negative integer means a minimal number of arguments.
    /// Command arity always includes the command's name itself (and the subcommand when applicable)
    arity: i16,
    first_key: i16,
    last_key: i16,
    step: u16,
}

impl CommandMetadata {
    pub fn new(cmd_name: RedisCommandName) -> Self {
        CommandMetadata {
            cmd_name,
            cmd_flags: RedisCommandFlags::None as u64,
            arity: 2,
            first_key: 1,
            last_key: 1,
            step: 1,
        }
    }

    /// Arity is the number of arguments a command expects. It follows a simple pattern:
    /// - A positive integer means a fixed number of arguments.
    /// - A negative integer means a minimal number of arguments.
    /// Command arity always includes the command's name itself (and the subcommand when applicable).
    pub fn with_arity(mut self, arity: i16) -> Self {
        self.arity = arity;
        self
    }
    /// The step, or increment, between the first key and the position of the next key.
    pub fn with_step(mut self, step: u16) -> Self {
        self.step = step;
        self
    }

    /// The position of the command's first key name argument. For most commands, the first key's position is 1.
    /// Position 0 is always the command name itself.
    pub fn with_first_key(mut self, first_key: i16) -> Self {
        self.first_key = first_key;
        self
    }

    /// The position of the command's last key name argument. Redis commands usually accept one, two or multiple
    /// number of keys.Commands that accept a single key have both first key and last key set to 1.
    /// Commands that accept two key name arguments, e.g. BRPOPLPUSH, SMOVE and RENAME, have this value set to
    /// the position of their second key. Multi-key commands that accept an arbitrary number of keys,
    /// such as MSET, use the value -1.
    pub fn with_last_key(mut self, last_key: i16) -> Self {
        self.last_key = last_key;
        self
    }

    /// This command might block the client
    pub fn blocking(mut self) -> Self {
        self.set_flag(RedisCommandFlags::Blocking);
        self
    }

    /// This command is a "write" command
    pub fn write(mut self) -> Self {
        self.set_flag(RedisCommandFlags::Write);
        self
    }

    /// This command performs "read" on the database
    pub fn read_only(mut self) -> Self {
        self.set_flag(RedisCommandFlags::Read);
        self
    }

    /// An administrator command
    pub fn admin(mut self) -> Self {
        self.set_flag(RedisCommandFlags::Admin);
        self
    }

    /// This command falls under the @connection category
    pub fn connection(mut self) -> Self {
        self.cmd_flags |= RedisCommandFlags::Connection as u64;
        self
    }

    pub fn name(&self) -> &RedisCommandName {
        &self.cmd_name
    }

    /// Is this command a "Write" command?
    pub fn is_write_command(&self) -> bool {
        self.cmd_flags & RedisCommandFlags::Write as u64 == RedisCommandFlags::Write as u64
    }

    pub fn to_resp_v2(&self) -> BytesMut {
        let builder = crate::RespBuilderV2::default();
        let mut buffer = BytesMut::with_capacity(64);

        let mut flags = Vec::<&str>::new();
        if self.has_flag(RedisCommandFlags::Read) {
            flags.push("readonly");
        }
        if self.has_flag(RedisCommandFlags::Write) {
            flags.push("write");
        }
        if self.has_flag(RedisCommandFlags::Blocking) {
            flags.push("blocking");
        }
        if self.has_flag(RedisCommandFlags::Admin) {
            flags.push("admin");
        }
        if self.has_flag(RedisCommandFlags::Connection) {
            flags.push("connection");
        }

        let cmdname = BytesMut::from(format!("{:?}", self.cmd_name).to_lowercase().as_str());

        // convert this object into RESP
        builder.add_array_len(&mut buffer, 10);
        builder.add_bulk_string(&mut buffer, &cmdname); // command name
        builder.add_number::<i16>(&mut buffer, self.arity, false); // arity
        builder.add_strings(&mut buffer, &flags); // array of flags
        builder.add_number::<i16>(&mut buffer, self.first_key, false); // first key
        builder.add_number::<i16>(&mut buffer, self.last_key, false); // last key
        builder.add_number::<u16>(&mut buffer, self.step, false); // step between keys
        builder.add_array_len(&mut buffer, 0); // ACL
        builder.add_array_len(&mut buffer, 0); // Tips
        builder.add_array_len(&mut buffer, 0); // Key specs
        builder.add_array_len(&mut buffer, 0); // Sub commands
        buffer
    }

    fn set_flag(&mut self, flag: RedisCommandFlags) {
        self.cmd_flags |= flag as u64
    }

    fn has_flag(&self, flag: RedisCommandFlags) -> bool {
        let res = self.cmd_flags & flag.clone() as u64;
        res == flag as u64
    }
}

impl Default for CommandsManager {
    fn default() -> Self {
        CommandsManager {
            cmds: HashMap::from([
                (
                    "config",
                    CommandMetadata::new(RedisCommandName::Config)
                        .read_only()
                        .with_arity(-2)
                        .with_first_key(0)
                        .with_last_key(0)
                        .with_step(0),
                ),
                (
                    "info",
                    CommandMetadata::new(RedisCommandName::Info)
                        .read_only()
                        .with_arity(-1)
                        .with_first_key(0)
                        .with_last_key(0)
                        .with_step(0),
                ),
                // string commands
                (
                    "append",
                    CommandMetadata::new(RedisCommandName::Append)
                        .write()
                        .with_arity(3),
                ),
                (
                    "decr",
                    CommandMetadata::new(RedisCommandName::Decr)
                        .write()
                        .with_arity(2),
                ),
                (
                    "decrby",
                    CommandMetadata::new(RedisCommandName::DecrBy)
                        .write()
                        .with_arity(3),
                ),
                (
                    "incr",
                    CommandMetadata::new(RedisCommandName::Incr)
                        .write()
                        .with_arity(2),
                ),
                (
                    "incrby",
                    CommandMetadata::new(RedisCommandName::IncrBy)
                        .write()
                        .with_arity(3),
                ),
                (
                    "incrbyfloat",
                    CommandMetadata::new(RedisCommandName::IncrByFloat)
                        .write()
                        .with_arity(3),
                ),
                (
                    "set",
                    CommandMetadata::new(RedisCommandName::Set)
                        .write()
                        .with_arity(3),
                ),
                (
                    "get",
                    CommandMetadata::new(RedisCommandName::Get)
                        .read_only()
                        .with_arity(2),
                ),
                (
                    "getdel",
                    CommandMetadata::new(RedisCommandName::GetDel)
                        .write()
                        .with_arity(2),
                ),
                (
                    "getset",
                    CommandMetadata::new(RedisCommandName::GetSet)
                        .write()
                        .with_arity(3),
                ),
                (
                    "getex",
                    CommandMetadata::new(RedisCommandName::GetEx)
                        .write()
                        .with_arity(-2),
                ),
                (
                    "getrange",
                    CommandMetadata::new(RedisCommandName::GetRange)
                        .read_only()
                        .with_arity(4),
                ),
                (
                    "lcs",
                    CommandMetadata::new(RedisCommandName::Lcs)
                        .read_only()
                        .with_arity(-3)
                        .with_last_key(2),
                ),
                (
                    "mget",
                    CommandMetadata::new(RedisCommandName::Mget)
                        .read_only()
                        .with_arity(-2)
                        .with_last_key(-1),
                ),
                (
                    "mset",
                    CommandMetadata::new(RedisCommandName::Mset)
                        .write()
                        .with_arity(-3)
                        .with_last_key(-1)
                        .with_step(2),
                ),
                (
                    "msetnx",
                    CommandMetadata::new(RedisCommandName::Msetnx)
                        .write()
                        .with_arity(-3)
                        .with_last_key(-1)
                        .with_step(2),
                ),
                (
                    "psetex",
                    CommandMetadata::new(RedisCommandName::Psetex)
                        .write()
                        .with_arity(4),
                ),
                (
                    "setex",
                    CommandMetadata::new(RedisCommandName::Setex)
                        .write()
                        .with_arity(4),
                ),
                (
                    "setnx",
                    CommandMetadata::new(RedisCommandName::Setnx)
                        .write()
                        .with_arity(3),
                ),
                (
                    "setrange",
                    CommandMetadata::new(RedisCommandName::SetRange)
                        .write()
                        .with_arity(4),
                ),
                (
                    "strlen",
                    CommandMetadata::new(RedisCommandName::Strlen)
                        .read_only()
                        .with_arity(2),
                ),
                (
                    "substr",
                    CommandMetadata::new(RedisCommandName::Substr)
                        .read_only()
                        .with_arity(4),
                ),
                // list commands
                (
                    "lpush",
                    CommandMetadata::new(RedisCommandName::Lpush)
                        .write()
                        .with_arity(-3),
                ),
                (
                    "lpushx",
                    CommandMetadata::new(RedisCommandName::Lpushx)
                        .write()
                        .with_arity(-3),
                ),
                (
                    "rpush",
                    CommandMetadata::new(RedisCommandName::Rpush)
                        .write()
                        .with_arity(-3),
                ),
                (
                    "rpushx",
                    CommandMetadata::new(RedisCommandName::Rpushx)
                        .write()
                        .with_arity(-3),
                ),
                (
                    "lpop",
                    CommandMetadata::new(RedisCommandName::Lpop)
                        .write()
                        .with_arity(-2),
                ),
                (
                    "rpop",
                    CommandMetadata::new(RedisCommandName::Rpop)
                        .write()
                        .with_arity(-2),
                ),
                (
                    "llen",
                    CommandMetadata::new(RedisCommandName::Llen)
                        .read_only()
                        .with_arity(2),
                ),
                (
                    "lindex",
                    CommandMetadata::new(RedisCommandName::Lindex)
                        .read_only()
                        .with_arity(4),
                ),
                (
                    "linsert",
                    CommandMetadata::new(RedisCommandName::Linsert)
                        .write()
                        .with_arity(5),
                ),
                (
                    "lset",
                    CommandMetadata::new(RedisCommandName::Lset)
                        .write()
                        .with_arity(4),
                ),
                (
                    "lpos",
                    CommandMetadata::new(RedisCommandName::Lpos)
                        .read_only()
                        .with_arity(-3),
                ),
                (
                    "ltrim",
                    CommandMetadata::new(RedisCommandName::Ltrim)
                        .write()
                        .with_arity(4),
                ),
                (
                    "lrange",
                    CommandMetadata::new(RedisCommandName::Lrange)
                        .read_only()
                        .with_arity(4),
                ),
                (
                    "lrem",
                    CommandMetadata::new(RedisCommandName::Lrem)
                        .write()
                        .with_arity(4),
                ),
                (
                    "lmove",
                    CommandMetadata::new(RedisCommandName::Lmove)
                        .write()
                        .with_arity(5)
                        .with_last_key(2),
                ),
                (
                    "rpoplpush",
                    CommandMetadata::new(RedisCommandName::Rpoplpush)
                        .write()
                        .with_arity(3)
                        .with_last_key(2),
                ),
                (
                    "lmpop",
                    CommandMetadata::new(RedisCommandName::Lmpop)
                        .write()
                        .with_arity(-4)
                        .with_first_key(0)
                        .with_last_key(0)
                        .with_step(0),
                ),
                (
                    "brpoplpush",
                    CommandMetadata::new(RedisCommandName::Brpoplpush)
                        .write()
                        .blocking()
                        .with_arity(4)
                        .with_last_key(2),
                ),
                (
                    "blpop",
                    CommandMetadata::new(RedisCommandName::Blpop)
                        .write()
                        .blocking()
                        .with_arity(-3)
                        .with_last_key(-2),
                ),
                (
                    "blmove",
                    CommandMetadata::new(RedisCommandName::Blmove)
                        .write()
                        .blocking()
                        .with_arity(6)
                        .with_last_key(2),
                ),
                (
                    "blmpop",
                    CommandMetadata::new(RedisCommandName::Blmpop)
                        .write()
                        .blocking()
                        .with_arity(-5)
                        .with_first_key(0)
                        .with_last_key(0)
                        .with_step(0),
                ),
                (
                    "brpop",
                    CommandMetadata::new(RedisCommandName::Brpop)
                        .write()
                        .blocking()
                        .with_arity(-3)
                        .with_last_key(-2),
                ),
                // Client commands
                (
                    "client",
                    CommandMetadata::new(RedisCommandName::Client).connection(),
                ),
                (
                    "select",
                    CommandMetadata::new(RedisCommandName::Select)
                        .connection()
                        .with_arity(2)
                        .with_first_key(0)
                        .with_last_key(0)
                        .with_step(0),
                ),
                // Server commands
                (
                    "replicaof",
                    CommandMetadata::new(RedisCommandName::ReplicaOf)
                        .admin()
                        .with_arity(3)
                        .with_first_key(0)
                        .with_last_key(0)
                        .with_step(0),
                ),
                (
                    "slaveof",
                    CommandMetadata::new(RedisCommandName::SlaveOf)
                        .admin()
                        .with_arity(3)
                        .with_first_key(0)
                        .with_last_key(0)
                        .with_step(0),
                ),
                (
                    "ping",
                    CommandMetadata::new(RedisCommandName::Ping)
                        .read_only()
                        .with_arity(-1)
                        .with_first_key(0)
                        .with_last_key(0)
                        .with_step(0),
                ),
                (
                    "command",
                    CommandMetadata::new(RedisCommandName::Command)
                        .with_arity(-1)
                        .with_first_key(0)
                        .with_last_key(0)
                        .with_step(0),
                ),
                // generic commands
                (
                    "ttl",
                    CommandMetadata::new(RedisCommandName::Ttl)
                        .read_only()
                        .with_arity(2),
                ),
                (
                    "del",
                    CommandMetadata::new(RedisCommandName::Del)
                        .write()
                        .with_arity(-2)
                        .with_last_key(-1),
                ),
                (
                    "exists",
                    CommandMetadata::new(RedisCommandName::Exists)
                        .read_only()
                        .with_arity(-2)
                        .with_last_key(-1),
                ),
                (
                    "expire",
                    CommandMetadata::new(RedisCommandName::Expire)
                        .write()
                        .with_arity(-3),
                ),
                // Hash commands
                (
                    "hset",
                    CommandMetadata::new(RedisCommandName::Hset)
                        .write()
                        .with_arity(-4),
                ),
                (
                    "hmset",
                    CommandMetadata::new(RedisCommandName::Hmset)
                        .write()
                        .with_arity(-4),
                ),
                (
                    "hget",
                    CommandMetadata::new(RedisCommandName::Hget)
                        .read_only()
                        .with_arity(3),
                ),
                (
                    "hdel",
                    CommandMetadata::new(RedisCommandName::Hdel)
                        .write()
                        .with_arity(-3),
                ),
                (
                    "hlen",
                    CommandMetadata::new(RedisCommandName::Hlen)
                        .read_only()
                        .with_arity(2),
                ),
                (
                    "hexists",
                    CommandMetadata::new(RedisCommandName::Hexists)
                        .read_only()
                        .with_arity(3),
                ),
                (
                    "hgetall",
                    CommandMetadata::new(RedisCommandName::Hgetall)
                        .read_only()
                        .with_arity(2),
                ),
                (
                    "hincrbyfloat",
                    CommandMetadata::new(RedisCommandName::Hincrbyfloat)
                        .write()
                        .with_arity(4),
                ),
                (
                    "hincrby",
                    CommandMetadata::new(RedisCommandName::Hincrby)
                        .write()
                        .with_arity(4),
                ),
                (
                    "hkeys",
                    CommandMetadata::new(RedisCommandName::Hkeys)
                        .read_only()
                        .with_arity(2),
                ),
                (
                    "hvals",
                    CommandMetadata::new(RedisCommandName::Hvals)
                        .read_only()
                        .with_arity(2),
                ),
                (
                    "hmget",
                    CommandMetadata::new(RedisCommandName::Hmget)
                        .read_only()
                        .with_arity(-3),
                ),
                (
                    "hrandfield",
                    CommandMetadata::new(RedisCommandName::Hrandfield)
                        .read_only()
                        .with_arity(-2),
                ),
            ]),
        }
    }
}
