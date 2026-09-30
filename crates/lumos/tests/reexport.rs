//! Probes that upstream macros resolve through the facade re-exports.
//! If either fails to compile, the re-export story needs rethinking.

#[derive(lumos::serde::Serialize, lumos::serde::Deserialize, PartialEq, Debug)]
struct Probe {
    a: u32,
}

#[lumos::tokio::test]
async fn upstream_macros_resolve_through_facade_paths() {
    let probe = Probe { a: 1 };
    assert_eq!(serde_json::to_string(&probe).unwrap(), r#"{"a":1}"#);
}
