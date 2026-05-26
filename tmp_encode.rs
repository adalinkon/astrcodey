use astrcode_extension_sdk::s5r::*;
use serde_json::json;

fn main() {
    let msg = WireMessage::Initialize(InitializeMsg {
        id: "req-1".into(),
        protocol_version: "1.0".into(),
        peer: PeerInfo { name: "test".into(), role: "plugin".into(), version: None },
        handlers: vec![],
        provided_capabilities: vec![],
        metadata: json!({"protocol":{"s5r":"1.0"}}),
    });
    let encoded = encode_wire_message(&msg).unwrap();
    println!("{}", String::from_utf8_lossy(&encoded));
    let parsed = parse_wire_message(&encoded).unwrap();
    println!("{:?}", parsed);
}
