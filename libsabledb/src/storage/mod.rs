mod generic_db;
mod hash_db;
mod storage_adapter;
mod storage_rocksdb;
mod storage_trait;
mod string_db;
mod write_cache;

pub use crate::replication::{StorageUpdates, StorageUpdatesIterItem};
pub use crate::storage::storage_adapter::{
    BatchUpdate, PutFlags, StorageAdapter, StorageOpenParams,
};
pub use generic_db::GenericDb;
pub use hash_db::{
    GetHashMetadataResult, HashDb, HashDeleteResult, HashExistsResult, HashGetMultiResult,
    HashGetResult, HashLenResult, HashPutResult,
};
pub use storage_rocksdb::StorageRocksDb;
pub use storage_trait::{IterateCallback, StorageIterator, StorageTrait};
pub use string_db::StringsDb;
pub use write_cache::DbWriteCache;

#[macro_export]
macro_rules! storage_rocksdb {
    ($open_params:expr) => {{
        let mut db = $crate::storage::StorageAdapter::default();
        db.open($open_params).expect("rockdb open");
        db
    }};
}
