use crate::worker::{BroadcastMessageType, WorkerMessage, WorkerSender};
use crate::{
    replication::{
        ReplicationConfig, ReplicationWorkerMessage, Replicator, ReplicatorContext, ServerRole,
    },
    Client, SableError, ServerOptions, StorageAdapter, Telemetry, WorkerContext, WorkerManager,
};
use bytes::BytesMut;
use crossbeam::queue::SegQueue;
#[allow(unused_imports)]
use dashmap::{DashMap, DashSet};
use std::sync::Arc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tokio::sync::mpsc::Receiver as TokioReceiver;
use tokio::sync::mpsc::Sender as TokioSender;

// contains a table that maps a clientId -> Sender channel
type ChannelQueue = Arc<SegQueue<TokioSender<u8>>>;

// Contains a table that maps between a `Key` and a list of channels (FIFO)
type BlockedClientTable = DashMap<BytesMut, ChannelQueue>;

pub struct ServerState {
    blocked_clients: BlockedClientTable,
    telemetry: Arc<Mutex<Telemetry>>,
    opts: ServerOptions,
    role_primary: AtomicBool,
    replicator_context: Option<Arc<ReplicatorContext>>,
    worker_tx_channels: DashMap<std::thread::ThreadId, WorkerSender>,
}

#[allow(dead_code)]
pub struct Server {
    state: Arc<ServerState>,
    worker_manager: WorkerManager,
}

impl Default for ServerState {
    fn default() -> Self {
        ServerState::new()
    }
}

impl ServerState {
    pub fn new() -> Self {
        ServerState {
            telemetry: Arc::new(Mutex::new(Telemetry::default())),
            blocked_clients: BlockedClientTable::new(),
            opts: ServerOptions::default(),
            role_primary: AtomicBool::new(true),
            replicator_context: None,
            worker_tx_channels: DashMap::<std::thread::ThreadId, WorkerSender>::new(),
        }
    }

    pub fn add_worker_tx_channel(&self, worker_id: std::thread::ThreadId, tx: WorkerSender) {
        self.worker_tx_channels.insert(worker_id, tx);
    }

    /// Broadcast a message to all the workers
    pub async fn broadcast_msg(&self, message: BroadcastMessageType) -> Result<(), SableError> {
        for item in &self.worker_tx_channels {
            item.value()
                .send(WorkerMessage::BroadcastMessage(message))
                .await?;
        }
        Ok(())
    }

    pub fn set_server_options(mut self, opts: ServerOptions) -> Self {
        self.opts = opts;
        match self.opts.load_replication_config().role {
            ServerRole::Primary => self.set_primary(),
            ServerRole::Replica => self.set_replica(),
        }
        self
    }

    pub fn set_replication_context(mut self, replication_context: ReplicatorContext) -> Self {
        self.replicator_context = Some(Arc::new(replication_context));
        self
    }

    /// Mark client as "terminated"
    pub async fn terminate_client(&self, client_id: u128) -> Result<(), SableError> {
        // first, try to local thread, if this fails, broadcast the message to other threads
        if !Client::terminate_client(client_id) {
            self.broadcast_msg(BroadcastMessageType::KillClient(client_id))
                .await?;
        }
        Ok(())
    }

    pub fn shared_telemetry(&self) -> Arc<Mutex<Telemetry>> {
        self.telemetry.clone()
    }

    pub fn options(&self) -> &ServerOptions {
        &self.opts
    }

    /// Is the server role is primary?
    pub fn is_primary(&self) -> bool {
        self.role_primary.load(Ordering::Relaxed)
    }

    /// Is the server role is replica?
    pub fn is_replica(&self) -> bool {
        !self.is_primary()
    }

    pub fn set_replica(&self) {
        tracing::info!("Server marked as Replica");
        self.role_primary.store(false, Ordering::Relaxed);
    }

    pub fn set_primary(&self) {
        tracing::info!("Server marked as Primary");
        self.role_primary.store(true, Ordering::Relaxed);
    }

    /// If we have blocked clients waiting for `key` -> wake them up now
    pub async fn wakeup_clients(&self, key: &BytesMut, mut num_clients: usize) {
        tracing::debug!("waking up {} client(s) for key: {:?}", num_clients, key);
        if self.blocked_clients.is_empty() {
            tracing::debug!("there are no blocked clients");
            return;
        }

        // based on num_clients, wakeup all the clients that are blocked by
        // this key
        let remove_key = if let Some(channel_queue) = self.blocked_clients.get(key) {
            while num_clients > 0 {
                if let Some(client_channel) = channel_queue.pop() {
                    if let Err(e) = client_channel.send(0u8).await {
                        tracing::debug!(
                            "error while sending wakeup bit. client already timed out. {:?}",
                            e
                        );
                    } else {
                        num_clients = num_clients.saturating_sub(1);
                    }
                } else {
                    // No more clients in the queue
                    break;
                }
            }
            channel_queue.is_empty()
        } else {
            false
        };

        if remove_key {
            // we no longer have clients blocked by this key
            let _ = self.blocked_clients.remove(key);
        }
    }

    /// Block the current client for the provided keys
    pub async fn block_client(&self, keys: &[BytesMut]) -> TokioReceiver<u8> {
        tracing::debug!("blocking client for keys {:?}", keys);
        let (tx, rx) = tokio::sync::mpsc::channel(keys.len());

        for key in keys.iter() {
            if let Some(channel_queue) = self.blocked_clients.get(key) {
                channel_queue.push(tx.clone());
            } else {
                // first time
                let channel_queue = Arc::new(SegQueue::<TokioSender<u8>>::new());
                channel_queue.push(tx.clone());
                self.blocked_clients
                    .insert(key.clone(), channel_queue.clone());
            };
        }
        rx
    }

    // Connect to primary instance
    pub async fn connect_to_primary(&self, address: String, port: u16) -> Result<(), SableError> {
        if let Some(repliction_context) = &self.replicator_context {
            // Update the configurationf file first
            let repl_config = ReplicationConfig {
                role: ServerRole::Replica,
                ip: address,
                port,
            };
            ReplicationConfig::write_file(
                &repl_config,
                self.options().general_settings.config_dir.as_deref(),
            )?;

            repliction_context
                .send(ReplicationWorkerMessage::ConnectToPrimary)
                .await?;
            self.set_replica();
        }
        Ok(())
    }

    // Change the role of this instance to primary
    pub async fn switch_role_to_primary(&self) -> Result<(), SableError> {
        let repl_config = ReplicationConfig {
            role: ServerRole::Primary,
            ip: self
                .options()
                .general_settings
                .replication_listen_ip
                .to_string(),
            port: self.options().general_settings.port as u16 + 1000,
        };

        ReplicationConfig::write_file(
            &repl_config,
            self.options().general_settings.config_dir.as_deref(),
        )?;
        if let Some(repliction_context) = &self.replicator_context {
            repliction_context
                .send(ReplicationWorkerMessage::PrimaryMode)
                .await?;
            self.set_primary();
        }
        Ok(())
    }
}

impl Server {
    pub fn new(
        opts: ServerOptions,
        store: StorageAdapter,
        workers_count: usize,
    ) -> Result<Self, SableError> {
        let replicator_context = Replicator::run(opts.clone(), store.clone())?;
        let state = Arc::new(
            ServerState::new()
                .set_server_options(opts)
                .set_replication_context(replicator_context),
        );

        let worker_manager = WorkerManager::new(workers_count, store.clone(), state.clone())?;
        Ok(Server {
            state,
            worker_manager,
        })
    }

    pub fn get_worker(&self) -> &WorkerContext {
        self.worker_manager.pick()
    }
}
