//! Logging helper — routes progress output to stdout (CLI mode) or stderr
//! (MCP mode, where stdout is reserved for JSON-RPC).

use std::sync::atomic::{AtomicBool, Ordering};

static MCP_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_mcp_mode(mcp: bool) {
    MCP_MODE.store(mcp, Ordering::Relaxed);
}

pub fn is_mcp() -> bool {
    MCP_MODE.load(Ordering::Relaxed)
}

/// Progress/info output. In MCP mode goes to stderr so it never pollutes
/// the JSON-RPC stream on stdout.
#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {{
        if $crate::log::is_mcp() {
            eprintln!($($arg)*);
        } else {
            println!($($arg)*);
        }
    }};
}

/// Warning output — always stderr.
#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {{
        eprintln!($($arg)*);
    }};
}
