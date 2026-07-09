use std::{
    hash::{Hash, Hasher},
    io::{Read, Write},
};

use anyhow::{Context, Result, bail};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub const ASSISTANT_STATE_FORMAT: i64 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectAssistantSnapshot {
    pub version: u32,
    #[serde(default)]
    pub active_conversation: u64,
    #[serde(default)]
    pub next_conversation_id: u64,
    #[serde(default)]
    pub next_conversation_number: u64,
    #[serde(default)]
    pub conversations: Vec<PersistedAssistantConversation>,
}

impl Default for ProjectAssistantSnapshot {
    fn default() -> Self {
        Self {
            version: ASSISTANT_STATE_FORMAT as u32,
            active_conversation: 0,
            next_conversation_id: 1,
            next_conversation_number: 1,
            conversations: Vec::new(),
        }
    }
}

impl ProjectAssistantSnapshot {
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        match serde_json::to_vec(self) {
            Ok(bytes) => bytes.hash(&mut hasher),
            Err(_) => self.conversations.len().hash(&mut hasher),
        }
        hasher.finish()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedAssistantConversation {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub history: Vec<PersistedChatMessage>,
    #[serde(default)]
    pub transcript: Vec<PersistedTranscriptEntry>,
    #[serde(default)]
    pub input: String,
    #[serde(default)]
    pub session_usage: PersistedUsage,
    #[serde(default)]
    pub last_usage: Option<PersistedUsage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistedRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedChatMessage {
    pub role: PersistedRole,
    #[serde(default)]
    pub content: Vec<PersistedContentBlock>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PersistedContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    OpaqueReasoning {
        reasoning: PersistedReasoningBlob,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PersistedReasoningBlob {
    None,
    Anthropic { blocks: Vec<serde_json::Value> },
    OpenAiCompat { reasoning_content: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PersistedTranscriptEntry {
    User {
        text: String,
    },
    Assistant {
        text: String,
    },
    Tool {
        summary: String,
        result: Option<String>,
        is_error: bool,
    },
    Notice {
        text: String,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedUsage {
    #[serde(default)]
    pub input: u32,
    #[serde(default)]
    pub output: u32,
    #[serde(default)]
    pub cache_read: u32,
    #[serde(default)]
    pub cache_write: u32,
}

pub(crate) fn save_assistant_state(
    db: &Connection,
    snapshot: &ProjectAssistantSnapshot,
) -> Result<()> {
    let json = serde_json::to_vec(snapshot).context("serialize assistant state")?;
    let payload = compress(&json)?;
    db.execute("delete from assistant_state", [])?;
    db.execute(
        "insert into assistant_state (id, format, payload, uncompressed_len, updated_at_ms) values (1, ?1, ?2, ?3, ?4)",
        params![
            ASSISTANT_STATE_FORMAT,
            payload,
            json.len() as i64,
            now_ms(),
        ],
    )?;
    Ok(())
}

pub(crate) fn load_assistant_state(db: &Connection) -> Result<ProjectAssistantSnapshot> {
    let Some((format, payload, uncompressed_len)) = db
        .query_row(
            "select format, payload, uncompressed_len from assistant_state where id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)? as usize,
                ))
            },
        )
        .optional()?
    else {
        return Ok(ProjectAssistantSnapshot::default());
    };
    if format != ASSISTANT_STATE_FORMAT {
        bail!("unsupported assistant state format {format}");
    }
    let json = decompress(&payload, uncompressed_len)?;
    let snapshot: ProjectAssistantSnapshot =
        serde_json::from_slice(&json).context("deserialize assistant state")?;
    if snapshot.version > ASSISTANT_STATE_FORMAT as u32 {
        bail!("unsupported assistant state version {}", snapshot.version);
    }
    Ok(snapshot)
}

fn compress(json: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(json)
        .context("compress assistant state")?;
    encoder.finish().context("finish assistant compression")
}

fn decompress(bytes: &[u8], uncompressed_len: usize) -> Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(bytes);
    let mut out = Vec::with_capacity(uncompressed_len);
    decoder
        .read_to_end(&mut out)
        .context("decompress assistant state")?;
    Ok(out)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
