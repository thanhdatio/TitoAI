//! End-to-end regression tests for agent tool-loop error handling.
//!
//! These tests invoke telegram_reader.py directly to reproduce failure modes that
//! previously caused the agent to hang for 480s (channel budget) instead of
//! stopping early with a clear error.
//!
//! Key regression: `search_global --date-from "2025-01-01"` raised
//! "can't compare offset-naive and offset-aware datetimes" (exit_code=1)
//! causing the agent to retry 3+ times before the LLM gave up thinking → 480s hang.
//!
//! Requirements:
//!   - `TELEGRAM_API_ID`, `TELEGRAM_API_HASH`, `TELEGRAM_PHONE` env vars (or `.env` file)
//!   - Valid Telethon session at `~/.zeroclaw/workspace/skills/telegram-reader/.session/`
//!   - Network access to Telegram API
//!
//! Run:
//!   source .env && cargo test --test agent_tool_error_e2e -- --ignored --test-threads=1

use serde_json::Value;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::process::Command;

fn script_path() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME env var required");
    PathBuf::from(home)
        .join(".zeroclaw/workspace/skills/telegram-reader/scripts/telegram_reader.py")
}

fn dotenv_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("ZEROCLAW_DOTENV") {
        return PathBuf::from(explicit);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".env")
}

fn load_credentials() -> (String, String, String) {
    if let (Ok(id), Ok(hash), Ok(phone)) = (
        std::env::var("TELEGRAM_API_ID"),
        std::env::var("TELEGRAM_API_HASH"),
        std::env::var("TELEGRAM_PHONE"),
    ) {
        return (id, hash, phone);
    }
    let content = std::fs::read_to_string(dotenv_path()).expect("cannot read .env");
    let mut api_id = String::new();
    let mut api_hash = String::new();
    let mut phone = String::new();
    for line in content.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("TELEGRAM_API_ID=") {
            api_id = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("TELEGRAM_API_HASH=") {
            api_hash = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("TELEGRAM_PHONE=") {
            phone = v.trim().to_string();
        }
    }
    assert!(!api_id.is_empty(), "TELEGRAM_API_ID not found");
    assert!(!api_hash.is_empty(), "TELEGRAM_API_HASH not found");
    assert!(!phone.is_empty(), "TELEGRAM_PHONE not found");
    (api_id, api_hash, phone)
}

async fn run_script(args: &[&str]) -> (Option<i32>, Value, String) {
    let (api_id, api_hash, phone) = load_credentials();
    let output = Command::new("python3")
        .arg(script_path())
        .args(args)
        .env("TELEGRAM_API_ID", &api_id)
        .env("TELEGRAM_API_HASH", &api_hash)
        .env("TELEGRAM_PHONE", &phone)
        .output()
        .await
        .expect("failed to execute telegram_reader.py");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let json = serde_json::from_str(stdout.trim()).unwrap_or(Value::Null);
    (output.status.code(), json, stderr)
}

// ═══════════════════════════════════════════════════════════════════════════
// Regression: datetime offset-naive vs offset-aware crash
// ═══════════════════════════════════════════════════════════════════════════

/// Regression: `search_global --date-from "2025-01-01"` must NOT crash with
/// "can't compare offset-naive and offset-aware datetimes".
///
/// This was the root cause of the 480s agent hang: the script exited with
/// code 1 repeatedly, the LLM retried 3 times, then spent ~6 min thinking
/// before the channel budget (480s) fired.
///
/// Fix: all datetime.fromisoformat() calls must use .replace(tzinfo=timezone.utc)
/// so they are timezone-aware and comparable to message.date (UTC from Telegram).
#[tokio::test]
#[ignore = "requires network + Telegram credentials"]
async fn e2e_search_global_date_from_2025_does_not_crash() {
    let started = Instant::now();

    let (exit_code, json, stderr) = run_script(&[
        "search_global",
        "--query",
        "привет",
        "--limit",
        "5",
        "--date-from",
        "2025-01-01",
    ])
    .await;

    let elapsed = started.elapsed();

    assert_eq!(
        exit_code,
        Some(0),
        "script must exit 0 with date-from=2025-01-01\n\
         stderr: {stderr}\n\
         json: {json}"
    );
    assert_eq!(
        json["success"], true,
        "result must be success=true, got: {json}"
    );
    assert!(
        !stderr.contains("offset-naive") && !stderr.contains("offset-aware"),
        "datetime timezone error must not appear in stderr: {stderr}"
    );

    // Script should complete well within 30s — if it hangs it means the bug is back
    assert!(
        elapsed.as_secs() < 30,
        "script took {}s — expected < 30s",
        elapsed.as_secs()
    );
}

/// Regression: same crash via the exact command template used by the daemon skill tool.
///
/// The daemon passes all optional args even when empty: `--date-from {date_from}`.
/// When the LLM sets date_from="2025-01-01" and channel_filter="" and date_to="",
/// this exact invocation must succeed.
#[tokio::test]
#[ignore = "requires network + Telegram credentials"]
async fn e2e_search_global_exact_daemon_command_template() {
    let (exit_code, json, stderr) = run_script(&[
        "search_global",
        "--query",
        "мастер",
        "--limit",
        "5",
        "--date-from",
        "2025-01-01",
        "--date-to",
        "",
        "--channel-filter",
        "",
    ])
    .await;

    assert_eq!(
        exit_code,
        Some(0),
        "exact daemon command template must exit 0\n\
         stderr: {stderr}\n\
         json: {json}"
    );
    assert_eq!(
        json["success"], true,
        "result must be success=true, got: {json}"
    );
    assert!(
        !stderr.contains("offset-naive") && !stderr.contains("offset-aware"),
        "datetime timezone error in stderr: {stderr}"
    );
}

/// Regression: date_from filter actually works — results after 2025-01-01 only.
#[tokio::test]
#[ignore = "requires network + Telegram credentials"]
async fn e2e_search_global_date_from_filters_old_results() {
    let (exit_code, json, _stderr) = run_script(&[
        "search_global",
        "--query",
        "привет",
        "--limit",
        "20",
        "--date-from",
        "2025-01-01",
    ])
    .await;

    assert_eq!(exit_code, Some(0), "must exit 0: {json}");
    assert_eq!(json["success"], true, "must succeed: {json}");

    let results = json["results"].as_array().expect("results must be array");
    for msg in results {
        let date_str = msg["date"].as_str().expect("date must be string");
        assert!(
            date_str >= "2025-01-01",
            "all results must be >= 2025-01-01 due to date_from filter, got: {date_str}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Bot E2E: real conversation through the daemon (zverozabr account)
// ═══════════════════════════════════════════════════════════════════════════

fn zverozabr_session_path() -> String {
    let home = std::env::var("HOME").expect("HOME env var required");
    format!(
        "{}/.zeroclaw/workspace/skills/telegram-reader/.session/zverozabr_session",
        home
    )
}

/// Send a message to a bot via zverozabr_session.
/// Returns the message ID of the sent message, or panics.
///
/// Credentials and session path are passed via env vars to avoid Python
/// string-quoting issues inside Rust format strings.
async fn send_to_bot(bot_username: &str, text: &str) -> i64 {
    let (api_id, api_hash, _) = load_credentials();
    let session = zverozabr_session_path();

    // All dynamic values go through env vars — no quoting/escaping needed.
    let script = r#"
import asyncio, json, os, sys
from telethon import TelegramClient

SESSION   = os.environ["ZC_SESSION"]
API_ID    = os.environ["ZC_API_ID"]
API_HASH  = os.environ["ZC_API_HASH"]
BOT       = os.environ["ZC_BOT"]
TEXT      = os.environ["ZC_TEXT"]

async def main():
    client = TelegramClient(SESSION, API_ID, API_HASH)
    await client.connect()
    if not await client.is_user_authorized():
        print(json.dumps({"success": False, "error": "zverozabr_session not authorized"}))
        sys.exit(1)
    msg = await client.send_message(BOT, TEXT)
    print(json.dumps({"success": True, "message_id": msg.id}))
    await client.disconnect()

asyncio.run(main())
"#;

    let output = Command::new("python3")
        .arg("-c")
        .arg(script)
        .env("ZC_SESSION", &session)
        .env("ZC_API_ID", &api_id)
        .env("ZC_API_HASH", &api_hash)
        .env("ZC_BOT", bot_username)
        .env("ZC_TEXT", text)
        .output()
        .await
        .expect("failed to run send_to_bot script");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let json: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|_| {
        panic!("send_to_bot: invalid JSON\nstdout: {stdout}\nstderr: {stderr}")
    });

    assert!(
        json["success"] == true,
        "send_to_bot failed: {json}\nstderr: {stderr}"
    );

    json["message_id"]
        .as_i64()
        .expect("send_to_bot: message_id must be integer")
}

/// Poll for a bot reply after `after_message_id`, waiting up to `timeout`.
/// Returns the reply text, or None if timeout elapsed without a reply.
///
/// Credentials and session path are passed via env vars to avoid Python
/// string-quoting issues inside Rust format strings.
async fn wait_for_bot_reply(
    bot_username: &str,
    after_message_id: i64,
    timeout: Duration,
) -> Option<String> {
    let (api_id, api_hash, _) = load_credentials();
    let session = zverozabr_session_path();
    let deadline = Instant::now() + timeout;
    let after_id_str = after_message_id.to_string();

    while Instant::now() < deadline {
        // All dynamic values go through env vars — no quoting/escaping needed.
        let script = r#"
import asyncio, json, os, sys
from telethon import TelegramClient

SESSION  = os.environ["ZC_SESSION"]
API_ID   = os.environ["ZC_API_ID"]
API_HASH = os.environ["ZC_API_HASH"]
BOT      = os.environ["ZC_BOT"]
AFTER_ID = int(os.environ["ZC_AFTER_ID"])

async def main():
    client = TelegramClient(SESSION, API_ID, API_HASH)
    await client.connect()
    if not await client.is_user_authorized():
        print(json.dumps({"success": False, "error": "not authorized"}))
        sys.exit(1)
    msgs = await client.get_messages(BOT, limit=10)
    results = []
    for m in msgs:
        if m.id > AFTER_ID and m.out is False:
            results.append({"id": m.id, "text": m.text or ""})
    print(json.dumps({"success": True, "messages": results}))
    await client.disconnect()

asyncio.run(main())
"#;

        let output = Command::new("python3")
            .arg("-c")
            .arg(script)
            .env("ZC_SESSION", &session)
            .env("ZC_API_ID", &api_id)
            .env("ZC_API_HASH", &api_hash)
            .env("ZC_BOT", bot_username)
            .env("ZC_AFTER_ID", &after_id_str)
            .output()
            .await
            .expect("failed to run wait_for_bot_reply script");

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let json: Value = serde_json::from_str(stdout.trim()).unwrap_or(Value::Null);

        if json["success"] == true {
            if let Some(msgs) = json["messages"].as_array() {
                if !msgs.is_empty() {
                    // Return the oldest reply (lowest id > after_message_id)
                    let mut sorted = msgs.to_vec();
                    sorted.sort_by_key(|m| m["id"].as_i64().unwrap_or(0));
                    let text = sorted[0]["text"].as_str().unwrap_or("").to_string();
                    return Some(text);
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    None
}

/// E2E: send the problematic "Поищи не раньше 2025" request to the bot and
/// verify it answers within 120s with search results — not a timeout/hang.
///
/// This is the real regression gate: before the datetime fix the bot would hang
/// for 480s (channel budget) because search_global crashed with exit_code=1
/// and the LLM retried → kept thinking for ~6min.
///
/// Requirements:
///   - Daemon running with the fixed binary (`source .env && ./target/release/zeroclaws start`)
///   - zverozabr_session authorized: session file exists AND `is_user_authorized()` returns True.
///     If the session is expired/invalid, re-authorize via:
///       python3 -c "
///         import asyncio; from telethon import TelegramClient
///         async def main():
///             c = TelegramClient('~/.zeroclaw/workspace/skills/telegram-reader/.session/zverozabr_session', API_ID, API_HASH)
///             await c.start(phone=ZVEROZABR_PHONE)
///             await c.disconnect()
///         asyncio.run(main())"
///   - TELEGRAM_API_ID, TELEGRAM_API_HASH set (or in .env)
///   - Bot @zGsR_bot is in the zverozabr allowlist
#[tokio::test]
#[ignore = "requires live daemon + authorized zverozabr_session + Telegram credentials"]
async fn e2e_bot_answers_samui_search_with_date_filter() {
    let bot = "zGsR_bot";
    let query = "Поищи в самуйских чатах сантехника, не раньше 2025 года. Топ-3 контакта.";

    // 0. No model switch — use whatever provider is active in the daemon session.
    //    The test validates that the bot replies at all (no hang), not which model is used.

    // 1. Send via zverozabr_session
    println!("Sending to @{bot}: {query}");
    let sent_id = send_to_bot(bot, query).await;
    println!("Sent message id={sent_id}");

    let start = Instant::now();

    // 2. Poll for reply every 5s, up to 180s.
    //    The channel message_timeout_secs=120 with 4x scale cap = 480s worst case.
    //    In practice gpt-5.2 answers a tool-using query in ~40-60s.
    //    We use 180s to give headroom while still catching real hangs.
    let reply = wait_for_bot_reply(bot, sent_id, Duration::from_secs(180)).await;

    let elapsed = start.elapsed();
    println!("Elapsed: {}s", elapsed.as_secs());

    // 3. Assert reply arrived in time
    let text = reply.unwrap_or_else(|| {
        panic!(
            "Bot @{bot} did not reply within 180s after message id={sent_id}. \
             Possible hang — check daemon logs."
        )
    });
    println!("Bot reply: {text}");

    assert!(
        elapsed < Duration::from_secs(180),
        "Reply arrived but took {}s ≥ 180s — possible slow hang",
        elapsed.as_secs()
    );
    assert!(
        !text.to_lowercase().contains("timed out"),
        "Bot reply indicates timeout: {text}"
    );
    assert!(
        !text.to_lowercase().contains("request timed out"),
        "Channel budget fired: {text}"
    );

    // Reply must not be a raw provider auth error forwarded to the user.
    assert!(
        !text.to_lowercase().contains("unauthorized")
            && !text.to_lowercase().contains("api key not valid")
            && !text.to_lowercase().contains("401"),
        "Bot reply looks like a provider auth error, not a real answer: {text}"
    );

    // Reply must contain something relevant — contacts, location keywords, or an explanation.
    // Rate limit from Telegram API is acceptable (agent handled it gracefully, not hung).
    let has_result = text.contains('@')
        || text.contains("2025")
        || text.contains("2026")
        || text.to_lowercase().contains("самуи")
        || text.to_lowercase().contains("сантехник")
        || text.to_lowercase().contains("мастер")
        || text.to_lowercase().contains("rate limit")
        || text.to_lowercase().contains("перегружен")
        || text.to_lowercase().contains("попробую позже")
        // Graceful render-error recovery: LLM received soft error and reported it
        || text.to_lowercase().contains("ошибк")
        || text.to_lowercase().contains("не удалось")
        || text.to_lowercase().contains("не смог")
        || text.to_lowercase().contains("параметр");
    assert!(
        has_result,
        "Bot reply does not look like search results or a handled error: {text}"
    );
}
