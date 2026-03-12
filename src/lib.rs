//! @milady/signal-native — Native Node.js Signal client.
//!
//! Wraps the Presage library (Rust Signal client) with napi-rs bindings.
//! All Presage async operations run on dedicated single-threaded runtimes
//! via spawn_blocking, because Presage futures are not Send.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::{pin_mut, StreamExt};
use napi::threadsafe_function::{ErrorStrategy, ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use presage::libsignal_service::configuration::SignalServers;
use presage::libsignal_service::content::ContentBody;
use presage::libsignal_service::protocol::ServiceId;
use presage::manager::Registered;
use presage::model::messages::Received;
use presage::store::ContentsStore;
use presage::Manager;
use presage_store_sqlite::{OnNewIdentity, SqliteStore};
use tokio::sync::{watch, Mutex};

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

type RegisteredManager = Manager<SqliteStore, Registered>;
type LinkResult = Result<RegisteredManager, String>;

struct ManagerEntry {
    manager: RegisteredManager,
    cancel_tx: Option<watch::Sender<bool>>,
}

static MANAGERS: once_cell::sync::Lazy<Mutex<HashMap<String, Arc<Mutex<ManagerEntry>>>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

static LINK_HANDLES: once_cell::sync::Lazy<
    Mutex<HashMap<String, tokio::task::JoinHandle<LinkResult>>>,
> = once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Run a (potentially non-Send) Presage future on a dedicated single-threaded
/// runtime. Presage internally uses non-Send types (ThreadRng, dyn trait
/// objects without Send bounds), so we cannot run its futures directly on the
/// napi multi-threaded tokio runtime.
async fn presage_spawn<F, Fut, R>(f: F) -> napi::Result<R>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = napi::Result<R>>,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| napi::Error::from_reason(format!("Runtime error: {e}")))?;
        rt.block_on(f())
    })
    .await
    .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
}

async fn open_store(data_path: &str) -> napi::Result<SqliteStore> {
    SqliteStore::open(data_path, OnNewIdentity::Trust)
        .await
        .map_err(|e| napi::Error::from_reason(format!("Failed to open store at {data_path}: {e}")))
}

async fn get_or_create_manager(
    data_path: &str,
) -> napi::Result<Arc<Mutex<ManagerEntry>>> {
    let mut managers = MANAGERS.lock().await;
    if let Some(entry) = managers.get(data_path) {
        return Ok(entry.clone());
    }

    let store = open_store(data_path).await?;
    let manager = Manager::load_registered(store)
        .await
        .map_err(|e| napi::Error::from_reason(format!("Failed to load registered manager: {e}")))?;

    let entry = Arc::new(Mutex::new(ManagerEntry {
        manager,
        cancel_tx: None,
    }));
    managers.insert(data_path.to_string(), entry.clone());
    Ok(entry)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Types exposed to JavaScript
// ---------------------------------------------------------------------------

#[napi(object)]
pub struct JsSendResult {
    pub timestamp: f64,
}

#[napi(object)]
pub struct JsContact {
    pub uuid: String,
    pub phone_number: Option<String>,
    pub name: Option<String>,
}

#[napi(object)]
pub struct JsGroup {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub members_count: u32,
}

#[napi(object)]
pub struct JsProfile {
    pub uuid: String,
    pub phone_number: Option<String>,
}

#[napi(object)]
#[derive(Clone)]
pub struct JsAttachment {
    pub content_type: String,
    pub file_name: Option<String>,
    pub size: Option<f64>,
}

#[napi(object)]
#[derive(Clone)]
pub struct JsIncomingMessage {
    pub sender_uuid: String,
    pub timestamp: f64,
    pub text: Option<String>,
    pub group_id: Option<String>,
    pub is_reaction: bool,
    pub reaction_emoji: Option<String>,
    pub reaction_target_timestamp: Option<f64>,
    pub attachments: Vec<JsAttachment>,
    pub is_queue_empty: bool,
}

// ---------------------------------------------------------------------------
// Device linking (secondary device, like Signal Desktop)
// ---------------------------------------------------------------------------

/// Link as a secondary device. Returns the provisioning URL to encode as QR.
/// Call `finishLink()` after the user scans the QR code.
#[napi]
pub async fn link_device(data_path: String, device_name: String) -> napi::Result<String> {
    let dp = data_path.clone();
    let (url_tx, url_rx) = futures::channel::oneshot::channel();

    // Spawn linking in background on a dedicated thread (non-Send future)
    let handle = tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let store = SqliteStore::open(&dp, OnNewIdentity::Trust)
                .await
                .map_err(|e| format!("Store error: {e}"))?;
            Manager::link_secondary_device(
                store,
                SignalServers::Production,
                device_name,
                url_tx,
            )
            .await
            .map_err(|e| format!("Linking error: {e}"))
        })
    });

    LINK_HANDLES.lock().await.insert(data_path, handle);

    // Wait for provisioning URL (arrives before linking completes)
    let url = url_rx
        .await
        .map_err(|_| napi::Error::from_reason("Linking cancelled before URL was received"))?;

    Ok(url.to_string())
}

/// Wait for device linking to complete after the user has scanned the QR code.
#[napi]
pub async fn finish_link(data_path: String) -> napi::Result<()> {
    let handle = LINK_HANDLES
        .lock()
        .await
        .remove(&data_path)
        .ok_or_else(|| napi::Error::from_reason("No active linking session"))?;

    let manager = handle
        .await
        .map_err(|e| napi::Error::from_reason(format!("Task error: {e}")))?
        .map_err(|e| napi::Error::from_reason(format!("Linking failed: {e}")))?;

    let entry = Arc::new(Mutex::new(ManagerEntry {
        manager,
        cancel_tx: None,
    }));
    MANAGERS.lock().await.insert(data_path, entry);

    Ok(())
}

// ---------------------------------------------------------------------------
// Registration (primary device) — stub for now
// ---------------------------------------------------------------------------

/// Register as a primary device. Not yet implemented — use linkDevice().
#[napi]
pub async fn register(
    _data_path: String,
    _phone_number: String,
    _use_voice: bool,
) -> napi::Result<()> {
    Err(napi::Error::from_reason(
        "Primary registration not yet supported. Use linkDevice() to link as secondary device.",
    ))
}

/// Confirm registration. Not yet implemented — use linkDevice().
#[napi]
pub async fn confirm_registration(_data_path: String, _code: String) -> napi::Result<()> {
    Err(napi::Error::from_reason(
        "Primary registration not yet supported. Use linkDevice() to link as secondary device.",
    ))
}

// ---------------------------------------------------------------------------
// Messaging
// ---------------------------------------------------------------------------

/// Send a text message to a recipient identified by their ServiceId string.
#[napi]
pub async fn send_message(
    data_path: String,
    recipient: String,
    text: String,
) -> napi::Result<JsSendResult> {
    presage_spawn(move || async move {
        let entry = get_or_create_manager(&data_path).await?;
        let mut guard = entry.lock().await;

        let service_id =
            ServiceId::parse_from_service_id_string(&recipient).ok_or_else(|| {
                napi::Error::from_reason(format!("Invalid recipient ServiceId: {recipient}"))
            })?;

        let timestamp = now_millis();
        let content = ContentBody::DataMessage(presage::proto::DataMessage {
            body: Some(text),
            timestamp: Some(timestamp),
            ..Default::default()
        });

        guard
            .manager
            .send_message(service_id, content, timestamp)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to send: {e}")))?;

        Ok(JsSendResult {
            timestamp: timestamp as f64,
        })
    })
    .await
}

/// Send a text message to a group identified by hex-encoded master key.
#[napi]
pub async fn send_group_message(
    data_path: String,
    group_master_key_hex: String,
    text: String,
) -> napi::Result<JsSendResult> {
    presage_spawn(move || async move {
        let entry = get_or_create_manager(&data_path).await?;
        let mut guard = entry.lock().await;

        let key_bytes = hex::decode(&group_master_key_hex)
            .map_err(|e| napi::Error::from_reason(format!("Invalid hex group key: {e}")))?;

        let timestamp = now_millis();
        let content = ContentBody::DataMessage(presage::proto::DataMessage {
            body: Some(text),
            timestamp: Some(timestamp),
            ..Default::default()
        });

        guard
            .manager
            .send_message_to_group(&key_bytes, content, timestamp)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to send group message: {e}")))?;

        Ok(JsSendResult {
            timestamp: timestamp as f64,
        })
    })
    .await
}

// ---------------------------------------------------------------------------
// Receiving messages
// ---------------------------------------------------------------------------

/// Start receiving messages. Creates a dedicated manager instance for receiving
/// (separate from sending) so both can operate concurrently on the same store.
#[napi]
pub async fn receive_messages(
    data_path: String,
    #[napi(ts_arg_type = "(message: JsIncomingMessage) => void")] callback: ThreadsafeFunction<
        JsIncomingMessage,
        ErrorStrategy::Fatal,
    >,
) -> napi::Result<()> {
    // Ensure send manager entry exists (for cancel_tx storage)
    let entry = get_or_create_manager(&data_path).await?;
    let (cancel_tx, cancel_rx) = watch::channel(false);
    entry.lock().await.cancel_tx = Some(cancel_tx);

    let dp = data_path.clone();

    // Spawn receive loop on dedicated thread with its own manager
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let store = match SqliteStore::open(&dp, OnNewIdentity::Trust).await {
                Ok(s) => s,
                Err(e) => {
                    log::error!("[signal-native] Failed to open recv store: {e}");
                    return;
                }
            };
            let mut recv_manager: RegisteredManager = match Manager::load_registered(store).await {
                Ok(m) => m,
                Err(e) => {
                    log::error!("[signal-native] Failed to load recv manager: {e}");
                    return;
                }
            };

            let messages = match recv_manager.receive_messages().await {
                Ok(s) => s,
                Err(e) => {
                    log::error!("[signal-native] Failed to start receiving: {e}");
                    return;
                }
            };
            pin_mut!(messages);
            let mut cancel_rx = cancel_rx;

            loop {
                tokio::select! {
                    _ = cancel_rx.changed() => {
                        if *cancel_rx.borrow() {
                            log::info!("[signal-native] Receive loop cancelled for {dp}");
                            break;
                        }
                    }
                    item = messages.next() => {
                        match item {
                            Some(Received::QueueEmpty) => {
                                callback.call(
                                    JsIncomingMessage {
                                        sender_uuid: String::new(),
                                        timestamp: 0.0,
                                        text: None,
                                        group_id: None,
                                        is_reaction: false,
                                        reaction_emoji: None,
                                        reaction_target_timestamp: None,
                                        attachments: vec![],
                                        is_queue_empty: true,
                                    },
                                    ThreadsafeFunctionCallMode::NonBlocking,
                                );
                            }
                            Some(Received::Contacts) => {
                                log::info!("[signal-native] Contacts synchronized");
                            }
                            Some(Received::Content(content)) => {
                                if let Some(msg) = content_to_js_message(&content) {
                                    callback.call(msg, ThreadsafeFunctionCallMode::NonBlocking);
                                }
                            }
                            None => {
                                log::info!("[signal-native] Stream ended for {dp}");
                                break;
                            }
                        }
                    }
                }
            }
        });
    });

    Ok(())
}

/// Stop the receive loop for the given data path.
#[napi]
pub async fn stop_receiving(data_path: String) -> napi::Result<()> {
    let managers = MANAGERS.lock().await;
    if let Some(entry) = managers.get(&data_path) {
        let mut guard = entry.lock().await;
        if let Some(tx) = guard.cancel_tx.take() {
            let _ = tx.send(true);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Contacts and groups
// ---------------------------------------------------------------------------

/// List all stored contacts.
#[napi]
pub async fn list_contacts(data_path: String) -> napi::Result<Vec<JsContact>> {
    presage_spawn(move || async move {
        let entry = get_or_create_manager(&data_path).await?;
        let guard = entry.lock().await;

        let contacts = guard
            .manager
            .store()
            .contacts()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to list contacts: {e}")))?;

        Ok(contacts
            .flatten()
            .map(|c| JsContact {
                uuid: c.uuid.to_string(),
                phone_number: c.phone_number.as_ref().map(|p| p.to_string()),
                name: if c.name.is_empty() {
                    None
                } else {
                    Some(c.name.clone())
                },
            })
            .collect())
    })
    .await
}

/// List all stored groups.
#[napi]
pub async fn list_groups(data_path: String) -> napi::Result<Vec<JsGroup>> {
    presage_spawn(move || async move {
        let entry = get_or_create_manager(&data_path).await?;
        let guard = entry.lock().await;

        let groups = guard
            .manager
            .store()
            .groups()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to list groups: {e}")))?;

        Ok(groups
            .flatten()
            .map(|(key, group)| JsGroup {
                id: hex::encode(key),
                name: group.title,
                description: group.description,
                members_count: group.members.len() as u32,
            })
            .collect())
    })
    .await
}

// ---------------------------------------------------------------------------
// Reactions
// ---------------------------------------------------------------------------

/// Send a reaction emoji to a specific message.
#[napi]
pub async fn send_reaction(
    data_path: String,
    recipient: String,
    emoji: String,
    target_timestamp: f64,
) -> napi::Result<()> {
    presage_spawn(move || async move {
        let entry = get_or_create_manager(&data_path).await?;
        let mut guard = entry.lock().await;

        let service_id =
            ServiceId::parse_from_service_id_string(&recipient).ok_or_else(|| {
                napi::Error::from_reason(format!("Invalid recipient: {recipient}"))
            })?;

        let timestamp = now_millis();
        let content = ContentBody::DataMessage(presage::proto::DataMessage {
            reaction: Some(presage::proto::data_message::Reaction {
                emoji: Some(emoji),
                target_sent_timestamp: Some(target_timestamp as u64),
                ..Default::default()
            }),
            timestamp: Some(timestamp),
            ..Default::default()
        });

        guard
            .manager
            .send_message(service_id, content, timestamp)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to send reaction: {e}")))?;

        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

/// Get the registered device's own profile information.
#[napi]
pub async fn get_profile(data_path: String) -> napi::Result<JsProfile> {
    presage_spawn(move || async move {
        let entry = get_or_create_manager(&data_path).await?;
        let guard = entry.lock().await;

        let reg = guard.manager.registration_data();

        Ok(JsProfile {
            uuid: reg.service_ids.aci.to_string(),
            phone_number: Some(reg.phone_number.to_string()),
        })
    })
    .await
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn content_to_js_message(
    content: &presage::libsignal_service::content::Content,
) -> Option<JsIncomingMessage> {
    let sender_uuid = content.metadata.sender.service_id_string();

    match &content.body {
        ContentBody::DataMessage(dm) => {
            let is_reaction = dm.reaction.is_some();
            let reaction_emoji = dm.reaction.as_ref().and_then(|r| r.emoji.clone());
            let reaction_target_ts = dm
                .reaction
                .as_ref()
                .and_then(|r| r.target_sent_timestamp)
                .map(|t| t as f64);

            let group_id = dm
                .group_v2
                .as_ref()
                .and_then(|g| g.master_key.as_ref().map(|k| hex::encode(k)));

            Some(JsIncomingMessage {
                sender_uuid,
                timestamp: dm.timestamp.unwrap_or(0) as f64,
                text: dm.body.clone(),
                group_id,
                is_reaction,
                reaction_emoji,
                reaction_target_timestamp: reaction_target_ts,
                attachments: dm
                    .attachments
                    .iter()
                    .map(|a| JsAttachment {
                        content_type: a.content_type.clone().unwrap_or_default(),
                        file_name: a.file_name.clone(),
                        size: a.size.map(|s| s as f64),
                    })
                    .collect(),
                is_queue_empty: false,
            })
        }
        _ => None,
    }
}
