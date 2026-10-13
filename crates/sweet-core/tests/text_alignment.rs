use sweet_core::{Document, RenderOptions};

#[test]
fn text_rows_center_their_content() {
    let doc = Document::new().unwrap();
    let row = doc.create_element("button").unwrap();
    doc.set_class_name(row, "flex h-6 w-full items-center gap-3 px-4 text-xs")
        .unwrap();
    doc.append_child(doc.body(), row).unwrap();
    for (class, text) in [
        ("w-5 shrink-0 text-right", "3"),
        ("truncate", "Chopin - Nocturne op.9 No.2"),
    ] {
        let span = doc.create_element("span").unwrap();
        doc.set_class_name(span, class).unwrap();
        doc.set_text_content(span, text).unwrap();
        doc.append_child(row, span).unwrap();
    }
    let frame = doc
        .render(&RenderOptions {
            width: 296.0,
            height: 100.0,
            ..Default::default()
        })
        .unwrap();
    for b in frame
        .boxes
        .iter()
        .filter(|b| b.id != row && b.id != doc.body())
    {
        assert!((b.y + b.height / 2.0 - 12.0).abs() < 0.6);
        assert!((b.height - 16.0).abs() < 0.6);
    }
}

#[test]
fn wrapped_content_sets_header_height_and_pushes_following_rows() {
    let doc = Document::new().unwrap();
    let header = doc.create_element("div").unwrap();
    doc.set_class_name(header, "flex items-center gap-2 p-3 w-[180px]")
        .unwrap();
    let text = doc.create_element("div").unwrap();
    doc.set_class_name(text, "min-w-0 flex-1 text-xs").unwrap();
    doc.set_text_content(text, "A name starts at the first song.")
        .unwrap();
    let control = doc.create_element("button").unwrap();
    doc.set_class_name(control, "shrink-0 p-2 text-xs").unwrap();
    doc.set_text_content(control, "Close").unwrap();
    doc.append_child(header, text).unwrap();
    doc.append_child(header, control).unwrap();
    doc.append_child(doc.body(), header).unwrap();
    let next = doc.create_element("div").unwrap();
    doc.set_text_content(next, "Next row").unwrap();
    doc.append_child(doc.body(), next).unwrap();
    let frame = doc
        .render(&RenderOptions {
            width: 240.0,
            height: 200.0,
            ..Default::default()
        })
        .unwrap();
    let bounds = |id| frame.boxes.iter().find(|b| b.id == id).unwrap();
    let h = bounds(header);
    let t = bounds(text);
    let c = bounds(control);
    assert!(t.height > 16.0, "subtitle must wrap");
    assert!((h.height - t.height.max(c.height) - 24.0).abs() < 1.0);
    assert!((t.y + t.height / 2.0 - c.y - c.height / 2.0).abs() < 1.0);
    assert!(t.x + t.width <= c.x - 7.0);
    assert!((bounds(next).y - h.y - h.height).abs() < 1.0);
}
