use flow_core::{domain::*, graph::*, providers::*};
use serde_json::json;

fn renderer() -> serde_json::Value {
    json!({"size":[192,160],"fragment":"vec4 flow_draw(vec2 uv) { return flow_primary; }",
        "uniforms":{"u_level":{"source":"property","name":"level"}}})
}

#[test]
fn renderer_contract_survives_provider_schema_and_graph_serialization() {
    let definition: Definition = serde_json::from_value(json!({
        "id":"fixture.gauge","owner":"fixture","behavior":"invoked",
        "inputs":[],"outputs":[],"capabilities":[],"renderer":renderer()
    }))
    .unwrap();
    let schema = definition.node_schema();
    assert_eq!(schema.renderer, definition.renderer);
    let graph = FlowNet {
        node_types: vec![schema],
        ..Default::default()
    };
    validate_registry(&graph).unwrap();
    let restored: FlowNet = serde_json::from_value(serde_json::to_value(&graph).unwrap()).unwrap();
    assert_eq!(restored.node_types[0].renderer, definition.renderer);
}

#[test]
fn invalid_shader_metadata_is_rejected_before_rendering() {
    for (key, value) in [
        ("size", json!([0, 100])),
        ("size", json!([100, 2048])),
        ("fragment", json!("")),
        ("fragment", json!(" ".repeat(65_537))),
        (
            "uniforms",
            json!({"flow_primary":{"source":"constant","value":1}}),
        ),
        (
            "uniforms",
            json!({"u_bad;":{"source":"constant","value":1}}),
        ),
        ("uniforms", json!({"u_bad":{"source":"property","name":""}})),
        (
            "uniforms",
            json!({"u_bad":{"source":"constant","value":1e100}}),
        ),
    ] {
        let mut value_contract = renderer();
        value_contract[key] = value;
        let contract: RendererContract = serde_json::from_value(value_contract).unwrap();
        assert!(contract.validate().is_err(), "{key}: {contract:?}");
    }
    let mut unknown = renderer();
    unknown["shader_typo"] = json!("ignored?");
    assert!(serde_json::from_value::<RendererContract>(unknown).is_err());
}
