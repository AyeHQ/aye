use aye_protocol::PROTOCOL_VERSION;

use super::*;

#[test]
fn source_files_turn_ranges_into_spans() {
    let source = "import pytest\n\n\ndef test_x():\n    pass\n";
    let parsed = ruff_python_parser::parse_module(source).unwrap();
    let lines = LineIndex::from_source_text(source);
    let path = Path::new("/repo/t.py");
    let file = SourceFile {
        rel: ProjectPathRef::as_written("t.py"),
        abs: path,
        source,
        module: parsed.syntax(),
        lines: &lines,
    };
    let func = file.module.body[1].as_function_def_stmt().unwrap();
    let span = file.span(func.name.range());
    assert_eq!(span.line, Some(4));
    assert_eq!(file.text(func.name.range()), "test_x");
    assert_eq!((span.start, span.end), (20, 26));
}

#[test]
fn the_engine_module_is_embedded() {
    assert_eq!(ENGINE_MODULE.name, "aye_engine");
    assert!(ENGINE_MODULE.source.contains("class EngineExecutor"));
    assert!(
        ENGINE_MODULE
            .source
            .contains(&format!("PROTOCOL = {PROTOCOL_VERSION}"))
    );
}
