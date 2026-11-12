use flow_host::config::load_from;
#[test]
fn kdl_settings_include_peers_override_defaults_and_reject_cycles_and_invalid_limits() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("limits.kdl"),
        "max-concurrent 2\nsource-interval-secs 4\n",
    )
    .unwrap();
    let path = dir.path().join("runtime.kdl");
    std::fs::write(&path,"include \"limits.kdl\"\nmax-concurrent 3\nhost-id \"my-host\"\nhost \"peer\" { transport \"unix\"; socket \"/tmp/peer.sock\"; }\n").unwrap();
    let settings = load_from(&path).unwrap();
    assert_eq!(settings.limits.max_concurrent, 3);
    assert_eq!(settings.limits.source_interval_secs, 4);
    assert_eq!(settings.host_id.as_deref(), Some("my-host"));
    assert!(settings.hosts.contains_key("peer"));
    std::fs::write(dir.path().join("limits.kdl"), "include \"runtime.kdl\"\n").unwrap();
    assert!(
        load_from(&path)
            .err()
            .unwrap()
            .to_string()
            .contains("cycle")
    );
    std::fs::write(&path, "max-concurrent 0\n").unwrap();
    assert!(load_from(&path).is_err());
    std::fs::write(&path, "unknown-runtime-option true\n").unwrap();
    assert!(load_from(&path).is_err());
}
