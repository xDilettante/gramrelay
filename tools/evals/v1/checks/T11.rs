#[test]
fn bold_ranges_use_utf16_code_units() {
    for (text, range, offset, length) in [("A🦀ó", 1..5, 1, 2), ("🦀xя", 4..7, 2, 2)] {
        let value = solution::solve(text, range).unwrap();
        assert_eq!(value.text(), text);
        assert_eq!(value.entities().len(), 1);
        let entity = &value.entities()[0];
        assert_eq!(entity.kind, "bold");
        assert_eq!((entity.offset, entity.length), (offset, length));
    }
    assert!(solution::solve("A🦀", 2..5).is_err());
}
