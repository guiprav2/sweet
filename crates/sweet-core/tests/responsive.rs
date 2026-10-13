use sweet_core::{Document, RenderOptions};

#[test]
fn breakpoints_override_base_classes_and_sort_by_width() {
    let doc = Document::new().unwrap();
    let node = doc.create_element("div").unwrap();
    doc.set_class_name(node, "lg:w-32 sm:w-16 w-8 h-8").unwrap();
    doc.append_child(doc.body(), node).unwrap();
    for (width, expected) in [
        (639., 32.),
        (640., 64.),
        (1023., 64.),
        (1024., 128.),
        (600., 32.),
    ] {
        let frame = doc
            .render(&RenderOptions {
                width,
                height: 100.,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            frame.boxes.iter().find(|b| b.id == node).unwrap().width,
            expected
        );
    }
}

#[test]
fn responsive_hidden_wins_over_later_flex_and_hides_fixed_children() {
    let doc = Document::new().unwrap();
    let parent = doc.create_element("div").unwrap();
    let child = doc.create_element("button").unwrap();
    doc.set_class_name(parent, "lg:hidden flex w-32 h-8")
        .unwrap();
    doc.set_class_name(child, "fixed top-0 right-0 w-8 h-8 bg-red-500")
        .unwrap();
    doc.append_child(parent, child).unwrap();
    doc.append_child(doc.body(), parent).unwrap();
    for (width, visible) in [(800., true), (1200., false), (800., true)] {
        let frame = doc
            .render(&RenderOptions {
                width,
                height: 100.,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(frame.boxes.iter().any(|b| b.id == child), visible);
        assert_eq!(frame.boxes.iter().any(|b| b.id == parent), visible);
    }
}

#[test]
fn responsive_grid_repositions_existing_video_node() {
    let doc = Document::new().unwrap();
    doc.set_class_name(doc.body(), "grid grid-cols-1 grid-rows-3 lg:grid-rows-1 lg:grid-cols-[min(352px,25vw)_1fr_min(296px,22vw)]").unwrap();
    let video = doc.create_element("video").unwrap();
    doc.set_class_name(
        video,
        "row-start-1 col-start-1 lg:col-start-2 w-full h-full",
    )
    .unwrap();
    doc.append_child(doc.body(), video).unwrap();
    for (width, x, w, h) in [
        (800., 0., 800., 300.),
        (1440., 352., 792., 900.),
        (800., 0., 800., 300.),
    ] {
        let frame = doc
            .render(&RenderOptions {
                width,
                height: 900.,
                ..Default::default()
            })
            .unwrap();
        let b = frame.boxes.iter().find(|b| b.id == video).unwrap();
        assert_eq!((b.x, b.width, b.height), (x, w, h));
    }
}
