use aye_protocol::PROTOCOL_VERSION;

use super::*;

#[test]
fn the_engine_module_is_embedded() {
    assert_eq!(ENGINE.names().collect::<Vec<_>>(), ["aye_engine"]);
    assert_eq!(ENGINE_MODULE.name, "aye_engine");
    assert!(ENGINE_MODULE.source.contains("class EngineExecutor"));
    assert!(
        ENGINE_MODULE
            .source
            .contains(&format!("PROTOCOL = {PROTOCOL_VERSION}"))
    );
}
