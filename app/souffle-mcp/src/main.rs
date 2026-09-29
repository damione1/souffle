//! `souffle-mcp`: a standalone MCP stdio server exposing Souffle's local
//! meeting/dictation database to any MCP client (Claude Desktop, Claude
//! Code, ...), independent of whether the Souffle app itself is running.
//!
//! All protocol traffic goes over stdout per the MCP stdio transport, so
//! startup diagnostics go to stderr only.

use rmcp::{ServiceExt, transport::stdio};
use souffle_mcp::db::{McpDb, resolve_db_path};
use souffle_mcp::server::SouffleMcpServer;

#[tokio::main]
async fn main() {
    let db_path = resolve_db_path();

    let db = match McpDb::open(&db_path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("souffle-mcp: {e}");
            std::process::exit(1);
        }
    };

    let server = SouffleMcpServer::new(db);

    let service = match server.serve(stdio()).await {
        Ok(service) => service,
        Err(e) => {
            eprintln!("souffle-mcp: failed to start MCP server: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = service.waiting().await {
        eprintln!("souffle-mcp: server error: {e}");
        std::process::exit(1);
    }
}
