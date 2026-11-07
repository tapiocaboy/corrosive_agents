//! Blockchain access layer.
//!
//! A tiny JSON-RPC 2.0 client plus the SVM (Solana-family) and EVM operations
//! the agents need, exposed two ways:
//!
//! - as plain async functions (`svm_slot`, `evm_balance_wei`, …), and
//! - as [`corrosive_agents`] **skills** (`solana_get_balance`,
//!   `evm_get_block_number`, …) the single-process agent calls through LLM
//!   tool-calling.
//!
//! Skills are **parametric over an endpoint**: `svm_skills(endpoint, network)`
//! captures the RPC URL and a network label, so one process serves exactly one
//! chain and the mesh runs one node per Solana-like network (Solana testnet,
//! devnet, mainnet, Eclipse, Sonic, …) plus EVM chains.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use corrosive_agents::skills::FnSkill;
use serde_json::{json, Value};

use crate::protocol::{ChainKind, ChainTag};

/// Lamports in one SOL.
pub const LAMPORTS_PER_SOL: f64 = 1_000_000_000.0;
/// Wei in one ETH.
pub const WEI_PER_ETH: f64 = 1_000_000_000_000_000_000.0;

/// A specialist's fully-resolved chain: what to announce (`tag`) plus the local
/// RPC `endpoint` (never gossiped).
#[derive(Debug, Clone)]
pub struct ChainConfig {
    /// Wire identity (kind + network name).
    pub tag: ChainTag,
    /// The JSON-RPC endpoint this node talks to.
    pub endpoint: String,
}

/// Built-in network presets: `name -> (kind, default endpoint)`.
pub const PRESETS: &[(&str, ChainKind, &str)] = &[
    ("solana-mainnet", ChainKind::Svm, "https://api.mainnet-beta.solana.com"),
    ("solana-testnet", ChainKind::Svm, "https://api.testnet.solana.com"),
    ("solana-devnet", ChainKind::Svm, "https://api.devnet.solana.com"),
    ("eclipse", ChainKind::Svm, "https://mainnetbeta-rpc.eclipse.xyz"),
    ("sonic-testnet", ChainKind::Svm, "https://api.testnet.sonic.game"),
    ("ethereum", ChainKind::Evm, "https://ethereum-rpc.publicnode.com"),
    ("base", ChainKind::Evm, "https://base-rpc.publicnode.com"),
    ("polygon", ChainKind::Evm, "https://polygon-bor-rpc.publicnode.com"),
];

/// Comma-separated list of preset names (for help text).
pub fn preset_names() -> String {
    PRESETS
        .iter()
        .map(|(n, _, _)| *n)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve the chain a specialist should serve from `token` (a preset name, or
/// a custom label when `CHAIN_KIND` + `RPC_URL` are set). `RPC_URL` always
/// overrides a preset's default endpoint.
pub fn resolve_chain(token: &str) -> Result<ChainConfig> {
    let token = token.trim();
    if let Some((_, kind, default_ep)) = PRESETS.iter().find(|(n, _, _)| n.eq_ignore_ascii_case(token)) {
        let endpoint = env_opt("RPC_URL").unwrap_or_else(|| default_ep.to_string());
        return Ok(ChainConfig {
            tag: ChainTag { kind: *kind, name: token.to_ascii_lowercase() },
            endpoint,
        });
    }
    // Custom chain: require RPC_URL, and a CHAIN_KIND (defaults to svm).
    let kind = match env_opt("CHAIN_KIND").as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("evm") | Some("eth") | Some("ethereum") => ChainKind::Evm,
        Some("svm") | Some("solana") | None => ChainKind::Svm,
        Some(other) => bail!("unknown CHAIN_KIND '{other}' (expected svm|evm)"),
    };
    let endpoint = env_opt("RPC_URL").ok_or_else(|| {
        anyhow!(
            "unknown chain '{token}'. Use a preset ({}) or set RPC_URL (and CHAIN_KIND=svm|evm).",
            preset_names()
        )
    })?;
    Ok(ChainConfig {
        tag: ChainTag { kind, name: token.to_ascii_lowercase() },
        endpoint,
    })
}

/// A minimal JSON-RPC 2.0 client over HTTPS.
#[derive(Clone)]
pub struct Rpc {
    url: String,
    http: reqwest::Client,
}

impl Rpc {
    /// Build a client for an explicit endpoint.
    pub fn new(url: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent("corrosive-agents-solana-usecase")
            .build()
            .expect("building a reqwest client");
        Self {
            url: url.into(),
            http,
        }
    }

    /// Perform a single JSON-RPC call and return its `result` value.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let resp = self
            .http
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {} ({method})", self.url))?;
        let status = resp.status();
        let payload: Value = resp
            .json()
            .await
            .with_context(|| format!("decoding {method} response ({status})"))?;
        if let Some(err) = payload.get("error") {
            return Err(anyhow!("RPC {method} error ({status}): {err}"));
        }
        payload
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow!("RPC {method} returned no result ({status})"))
    }
}

// ── SVM (Solana-family) operations ───────────────────────────────────────────

/// Current slot (the chain's height / liveness clock).
pub async fn svm_slot(rpc: &Rpc) -> Result<u64> {
    let v = rpc.call("getSlot", json!([{ "commitment": "confirmed" }])).await?;
    v.as_u64().ok_or_else(|| anyhow!("getSlot: unexpected {v}"))
}

/// Genesis hash — a base58 fingerprint unique to each SVM network. Proves which
/// network you are actually talking to (devnet ≠ testnet ≠ Eclipse ≠ Sonic).
pub async fn svm_genesis_hash(rpc: &Rpc) -> Result<String> {
    let v = rpc.call("getGenesisHash", json!([])).await?;
    v.as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("getGenesisHash: unexpected {v}"))
}

/// Balance of an account, in lamports.
pub async fn svm_balance_lamports(rpc: &Rpc, address: &str) -> Result<u64> {
    let v = rpc
        .call("getBalance", json!([address, { "commitment": "confirmed" }]))
        .await?;
    v.get("value")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("getBalance: unexpected {v}"))
}

/// Request a devnet/testnet airdrop (lamports from `sol`). Returns the tx
/// signature. Works on devnet/testnet only; mainnet returns an RPC error.
pub async fn svm_airdrop(rpc: &Rpc, address: &str, sol: f64) -> Result<String> {
    let lamports = (sol.max(0.0) * LAMPORTS_PER_SOL) as u64;
    let v = rpc.call("requestAirdrop", json!([address, lamports])).await?;
    v.as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("requestAirdrop: unexpected {v}"))
}

// ── EVM operations ──────────────────────────────────────────────────────────

/// Latest block number.
pub async fn evm_block_number(rpc: &Rpc) -> Result<u64> {
    let v = rpc.call("eth_blockNumber", json!([])).await?;
    parse_hex_u64(v.as_str().unwrap_or_default())
}

/// EIP-155 chain id (1 = Ethereum mainnet, 8453 = Base, 137 = Polygon, …).
pub async fn evm_chain_id(rpc: &Rpc) -> Result<u64> {
    let v = rpc.call("eth_chainId", json!([])).await?;
    parse_hex_u64(v.as_str().unwrap_or_default())
}

/// Balance of an address at the latest block, in wei.
pub async fn evm_balance_wei(rpc: &Rpc, address: &str) -> Result<u128> {
    let v = rpc.call("eth_getBalance", json!([address, "latest"])).await?;
    parse_hex_u128(v.as_str().unwrap_or_default())
}

// ── Conversions & parsing ────────────────────────────────────────────────────

/// Lamports → SOL (display precision; a demo, not an accounting ledger).
pub fn lamports_to_sol(lamports: u64) -> f64 {
    lamports as f64 / LAMPORTS_PER_SOL
}

/// Wei → ETH (display precision).
pub fn wei_to_eth(wei: u128) -> f64 {
    wei as f64 / WEI_PER_ETH
}

fn parse_hex_u64(s: &str) -> Result<u64> {
    let s = s.trim_start_matches("0x");
    u64::from_str_radix(if s.is_empty() { "0" } else { s }, 16)
        .with_context(|| format!("parsing hex u64 '{s}'"))
}

fn parse_hex_u128(s: &str) -> Result<u128> {
    let s = s.trim_start_matches("0x");
    u128::from_str_radix(if s.is_empty() { "0" } else { s }, 16)
        .with_context(|| format!("parsing hex u128 '{s}'"))
}

/// An env var, treated as absent when empty/whitespace.
pub fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

// ── Address detection (so a specialist can lift an address out of a question) ──

/// The first Solana-style base58 address (32–44 chars) found in free text.
pub fn find_solana_address(text: &str) -> Option<String> {
    const B58: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .find(|t| {
            let n = t.len();
            (32..=44).contains(&n) && t.chars().all(|c| B58.contains(c))
        })
        .map(str::to_string)
}

/// The first EVM `0x…` address (20 bytes) found in free text.
pub fn find_evm_address(text: &str) -> Option<String> {
    text.split(|c: char| !(c.is_ascii_hexdigit() || c == 'x' || c == 'X'))
        .find(|t| {
            t.len() == 42
                && (t.starts_with("0x") || t.starts_with("0X"))
                && t[2..].chars().all(|c| c.is_ascii_hexdigit())
        })
        .map(str::to_string)
}

// ── Skills (LLM tool-calling surface), parametric over the endpoint ──────────

/// The skills for one chain, keyed by its kind.
pub fn skills_for(cfg: &ChainConfig) -> Vec<FnSkill> {
    match cfg.tag.kind {
        ChainKind::Svm => svm_skills(&cfg.endpoint, &cfg.tag.name),
        ChainKind::Evm => evm_skills(&cfg.endpoint, &cfg.tag.name),
    }
}

/// SVM skills bound to one endpoint/network.
pub fn svm_skills(endpoint: &str, network: &str) -> Vec<FnSkill> {
    let ep = endpoint.to_string();
    let net = network.to_string();
    vec![
        {
            let (ep, net) = (ep.clone(), net.clone());
            FnSkill::new(
                "solana_get_slot",
                "Get the current slot (chain height / liveness) of the Solana/SVM network. No input.",
                move |_input| {
                    let (ep, net) = (ep.clone(), net.clone());
                    async move {
                        let rpc = Rpc::new(&ep);
                        let slot = svm_slot(&rpc).await.map_err(skill_err)?;
                        Ok(json!({ "network": net, "slot": slot, "endpoint": ep }))
                    }
                },
            )
        },
        {
            let (ep, net) = (ep.clone(), net.clone());
            FnSkill::new(
                "solana_get_genesis_hash",
                "Get the genesis hash — a fingerprint unique to each SVM network. No input.",
                move |_input| {
                    let (ep, net) = (ep.clone(), net.clone());
                    async move {
                        let rpc = Rpc::new(&ep);
                        let genesis = svm_genesis_hash(&rpc).await.map_err(skill_err)?;
                        Ok(json!({ "network": net, "genesis_hash": genesis, "endpoint": ep }))
                    }
                },
            )
        },
        {
            let (ep, net) = (ep.clone(), net.clone());
            FnSkill::new(
                "solana_get_balance",
                "Get the SOL balance of an SVM account. Input: {\"address\": \"<base58 pubkey>\"}.",
                move |input| {
                    let (ep, net) = (ep.clone(), net.clone());
                    async move {
                        let address = require_str(&input, "address")?;
                        let rpc = Rpc::new(&ep);
                        let lamports = svm_balance_lamports(&rpc, &address).await.map_err(skill_err)?;
                        Ok(json!({
                            "network": net,
                            "address": address,
                            "lamports": lamports,
                            "sol": lamports_to_sol(lamports),
                        }))
                    }
                },
            )
            .with_schema(json!({
                "type": "object",
                "properties": { "address": { "type": "string", "description": "base58 account address" } },
                "required": ["address"]
            }))
        },
        {
            let (ep, net) = (ep.clone(), net.clone());
            FnSkill::new(
                "solana_request_airdrop",
                "Request a devnet/testnet airdrop to an address (not available on mainnet). \
                 Input: {\"address\": \"<base58 pubkey>\", \"sol\": <number, default 1>}.",
                move |input| {
                    let (ep, net) = (ep.clone(), net.clone());
                    async move {
                        let address = require_str(&input, "address")?;
                        let sol = input.get("sol").and_then(Value::as_f64).unwrap_or(1.0);
                        let rpc = Rpc::new(&ep);
                        let signature = svm_airdrop(&rpc, &address, sol).await.map_err(skill_err)?;
                        Ok(json!({ "network": net, "address": address, "sol": sol, "signature": signature }))
                    }
                },
            )
            .with_schema(json!({
                "type": "object",
                "properties": { "address": { "type": "string" }, "sol": { "type": "number" } },
                "required": ["address"]
            }))
        },
    ]
}

/// EVM skills bound to one endpoint/network.
pub fn evm_skills(endpoint: &str, network: &str) -> Vec<FnSkill> {
    let ep = endpoint.to_string();
    let net = network.to_string();
    vec![
        {
            let (ep, net) = (ep.clone(), net.clone());
            FnSkill::new(
                "evm_get_block_number",
                "Get the latest EVM block number. No input.",
                move |_input| {
                    let (ep, net) = (ep.clone(), net.clone());
                    async move {
                        let rpc = Rpc::new(&ep);
                        let block = evm_block_number(&rpc).await.map_err(skill_err)?;
                        Ok(json!({ "network": net, "block_number": block, "endpoint": ep }))
                    }
                },
            )
        },
        {
            let (ep, net) = (ep.clone(), net.clone());
            FnSkill::new(
                "evm_get_chain_id",
                "Get the EVM chain id (1=Ethereum, 8453=Base, 137=Polygon). No input.",
                move |_input| {
                    let (ep, net) = (ep.clone(), net.clone());
                    async move {
                        let rpc = Rpc::new(&ep);
                        let chain_id = evm_chain_id(&rpc).await.map_err(skill_err)?;
                        Ok(json!({ "network": net, "chain_id": chain_id, "endpoint": ep }))
                    }
                },
            )
        },
        {
            let (ep, net) = (ep.clone(), net.clone());
            FnSkill::new(
                "evm_get_balance",
                "Get the native-coin balance of an EVM address. Input: {\"address\": \"0x…40 hex\"}.",
                move |input| {
                    let (ep, net) = (ep.clone(), net.clone());
                    async move {
                        let address = require_str(&input, "address")?;
                        let rpc = Rpc::new(&ep);
                        let wei = evm_balance_wei(&rpc, &address).await.map_err(skill_err)?;
                        Ok(json!({
                            "network": net,
                            "address": address,
                            "wei": wei.to_string(),
                            "eth": wei_to_eth(wei),
                        }))
                    }
                },
            )
            .with_schema(json!({
                "type": "object",
                "properties": { "address": { "type": "string", "description": "0x-prefixed 20-byte address" } },
                "required": ["address"]
            }))
        },
    ]
}

fn skill_err(e: anyhow::Error) -> corrosive_agents::Error {
    corrosive_agents::Error::Skill(e.to_string())
}

fn require_str(input: &Value, key: &str) -> corrosive_agents::Result<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            corrosive_agents::Error::Skill(format!("missing required string field '{key}'"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_a_solana_address() {
        let q = "balance of 9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM please";
        assert_eq!(
            find_solana_address(q).as_deref(),
            Some("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM")
        );
        assert_eq!(
            find_solana_address("0x742d35Cc6634C0532925a3b844Bc454e4438f44e"),
            None
        );
    }

    #[test]
    fn detects_an_evm_address() {
        let q = "what is the balance of 0x742d35Cc6634C0532925a3b844Bc454e4438f44e?";
        assert_eq!(
            find_evm_address(q).as_deref(),
            Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e")
        );
        assert_eq!(find_evm_address("no address here"), None);
    }

    #[test]
    fn converts_units() {
        assert_eq!(lamports_to_sol(1_500_000_000), 1.5);
        assert_eq!(wei_to_eth(1_000_000_000_000_000_000), 1.0);
    }

    #[test]
    fn resolves_presets_and_custom() {
        let testnet = resolve_chain("solana-testnet").unwrap();
        assert_eq!(testnet.tag.kind, ChainKind::Svm);
        assert_eq!(testnet.endpoint, "https://api.testnet.solana.com");

        let eclipse = resolve_chain("eclipse").unwrap();
        assert_eq!(eclipse.tag.kind, ChainKind::Svm);
        assert!(eclipse.endpoint.contains("eclipse"));

        // Unknown name with no RPC_URL is an error.
        assert!(resolve_chain("nope-chain").is_err());
    }

    #[test]
    fn preset_names_lists_every_preset() {
        let names = preset_names();
        assert!(names.contains("solana-testnet"));
        assert!(names.contains("eclipse"));
        assert!(names.contains("ethereum"));
    }

    #[test]
    fn env_opt_is_none_when_unset() {
        assert_eq!(env_opt("SOLANA_AGENTS_DEFINITELY_UNSET_VAR"), None);
    }

    #[test]
    fn parse_hex_handles_prefix_and_empty() {
        assert_eq!(parse_hex_u64("0x10").unwrap(), 16);
        assert_eq!(parse_hex_u64("1a").unwrap(), 26);
        assert_eq!(parse_hex_u64("").unwrap(), 0);
        assert_eq!(parse_hex_u128("0xde0b6b3a7640000").unwrap(), 1_000_000_000_000_000_000);
        assert!(parse_hex_u64("0xzz").is_err());
    }

    #[test]
    fn skills_for_dispatches_by_kind() {
        let svm = ChainConfig {
            tag: ChainTag { kind: ChainKind::Svm, name: "x".into() },
            endpoint: "http://127.0.0.1:1".into(),
        };
        let evm = ChainConfig {
            tag: ChainTag { kind: ChainKind::Evm, name: "y".into() },
            endpoint: "http://127.0.0.1:1".into(),
        };
        assert_eq!(skills_for(&svm).len(), 4);
        assert_eq!(skills_for(&evm).len(), 3);
    }

    #[tokio::test]
    async fn svm_rpc_functions_parse_mock_responses() {
        let url = crate::test_support::spawn_mock_rpc().await;
        let rpc = Rpc::new(&url);
        assert_eq!(svm_slot(&rpc).await.unwrap(), 123456);
        assert!(svm_genesis_hash(&rpc).await.unwrap().starts_with("MockGenesis"));
        assert_eq!(svm_balance_lamports(&rpc, "Addr").await.unwrap(), 2_500_000_000);
        assert!(svm_airdrop(&rpc, "Addr", 1.0).await.unwrap().starts_with("MockAirdrop"));
    }

    #[tokio::test]
    async fn evm_rpc_functions_parse_mock_responses() {
        let url = crate::test_support::spawn_mock_rpc().await;
        let rpc = Rpc::new(&url);
        assert_eq!(evm_block_number(&rpc).await.unwrap(), 16);
        assert_eq!(evm_chain_id(&rpc).await.unwrap(), 1);
        assert_eq!(evm_balance_wei(&rpc, "0xabc").await.unwrap(), 1_000_000_000_000_000_000);
    }

    #[tokio::test]
    async fn rpc_surfaces_json_rpc_errors() {
        let url = crate::test_support::spawn_mock_rpc().await;
        let rpc = Rpc::new(&url);
        let err = rpc.call("forceError", serde_json::json!([])).await.unwrap_err();
        assert!(err.to_string().contains("boom"));
    }

    #[tokio::test]
    async fn svm_skills_execute_against_mock() {
        use corrosive_agents::skills::Skill;
        let url = crate::test_support::spawn_mock_rpc().await;
        let skills = svm_skills(&url, "mock-svm");
        let names: Vec<&str> = skills.iter().map(|s| s.name()).collect();
        assert!(names.contains(&"solana_get_slot"));
        assert!(names.contains(&"solana_get_balance"));

        let slot_skill = skills.iter().find(|s| s.name() == "solana_get_slot").unwrap();
        let out = slot_skill.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(out["slot"], 123456);
        assert_eq!(out["network"], "mock-svm");

        let bal_skill = skills.iter().find(|s| s.name() == "solana_get_balance").unwrap();
        let out = bal_skill.execute(serde_json::json!({ "address": "Addr" })).await.unwrap();
        assert_eq!(out["sol"], 2.5);
        // Missing the required field is a skill error.
        assert!(bal_skill.execute(serde_json::json!({})).await.is_err());
    }

    #[tokio::test]
    async fn evm_skills_execute_against_mock() {
        use corrosive_agents::skills::Skill;
        let url = crate::test_support::spawn_mock_rpc().await;
        let skills = evm_skills(&url, "mock-evm");
        let block = skills.iter().find(|s| s.name() == "evm_get_block_number").unwrap();
        assert_eq!(block.execute(serde_json::json!({})).await.unwrap()["block_number"], 16);
        let bal = skills.iter().find(|s| s.name() == "evm_get_balance").unwrap();
        let out = bal.execute(serde_json::json!({ "address": "0xabc" })).await.unwrap();
        assert_eq!(out["eth"], 1.0);
    }
}
