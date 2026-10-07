//! Local IPC authentication: the engine serves its loopback RPC only to
//! clients presenting this install's secret. The implementation lives in
//! [`zeron_rpc::ipc_auth`] so clients that don't link the engine (the
//! injected `zeron mcp` server) share it; re-exported here for engine
//! callers.

pub use zeron_rpc::ipc_auth::*;
