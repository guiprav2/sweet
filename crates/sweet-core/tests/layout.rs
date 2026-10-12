use sweet_core::{Document, RenderOptions};

fn frame(doc: &Document, width: f32, height: f32) -> sweet_core::Frame {
    doc.render(&RenderOptions {
        width,
        height,
        scale: 1.0,
        hover: None,
        focus: None,
    })
    .unwrap()
}

fn pixel(frame: &sweet_core::Frame, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * frame.width + x) * 4) as usize;
    [
        frame.rgba[index],
        frame.rgba[index + 1],
        frame.rgba[index + 2],
        frame.rgba[index + 3],
    ]
}

fn near(actual: [u8; 4], expected: [u8; 3]) -> bool {
    actual[0].abs_diff(expected[0]) < 3
        && actual[1].abs_diff(expected[1]) < 3
        && actual[2].abs_diff(expected[2]) < 3
        && actual[3] == 255
}

#[test]
fn an_input_exposes_its_content_box() {
    let doc = Document::new().unwrap();
    let input = doc.create_element("input").unwrap();
    doc.set_class_name(input, "w-[120px] h-[32px] px-2 text-white")
        .unwrap();
    doc.set_attribute(input, "type", "password").unwrap();
    doc.set_control_value(input, "secret").unwrap();
    doc.append_child(doc.body(), input).unwrap();
    let frame = doc
        .render(&sweet_core::RenderOptions {
            width: 200.0,
            height: 80.0,
            scale: 1.0,
            hover: None,
            focus: None,
        })
        .unwrap();
    assert_eq!(frame.fields.len(), 1);
    assert!(frame.fields[0].password);
    assert_eq!(frame.fields[0].value, "secret");
    assert!(frame.fields[0].width < 120.0 && frame.fields[0].width > 100.0);
    let laid = frame.boxes.iter().find(|b| b.id == input).unwrap();
    assert!(
        (laid.height - 32.0).abs() < 1.0,
        "explicit height {}",
        laid.height
    );
}

#[test]
fn an_input_is_one_line_plus_padding() {
    let doc = Document::new().unwrap();
    let input = doc.create_element("input").unwrap();
    doc.set_class_name(input, "text-sm py-3 border w-[200px]")
        .unwrap();
    doc.append_child(doc.body(), input).unwrap();
    let frame = frame(&doc, 240.0, 80.0);
    let laid = frame.boxes.iter().find(|b| b.id == input).unwrap();
    // text-sm line box 20, py-3 is 12 + 12, border is 1 + 1.
    assert!((laid.height - 46.0).abs() < 1.5, "height {}", laid.height);
    let field = frame.fields.iter().find(|f| f.id == input).unwrap();
    assert!(
        (field.height - 20.0).abs() < 1.5,
        "content {}",
        field.height
    );
    assert!(field.clip_h + 0.5 >= field.height);
    assert!(field.clip_y <= field.y + 0.5);
}

#[test]
fn an_image_contains_inside_its_box() {
    let doc = Document::new().unwrap();
    let img = doc.create_element("img").unwrap();
    doc.set_class_name(img, "w-[8px] h-[4px] object-contain")
        .unwrap();
    let mut rgba = Vec::new();
    for _ in 0..2 {
        rgba.extend_from_slice(&[255, 0, 0, 255, 0, 0, 255, 255]);
    }
    doc.set_image_rgba(img, 2, 2, rgba).unwrap();
    doc.append_child(doc.body(), img).unwrap();
    let frame = frame(&doc, 8.0, 4.0);
    assert!(
        near(pixel(&frame, 0, 2), [255, 255, 255]),
        "letterbox {:?}",
        pixel(&frame, 0, 2)
    );
    assert!(
        near(pixel(&frame, 3, 2), [255, 0, 0]),
        "left {:?}",
        pixel(&frame, 3, 2)
    );
    assert!(
        near(pixel(&frame, 5, 2), [0, 0, 255]),
        "right {:?}",
        pixel(&frame, 5, 2)
    );
}

#[test]
fn background_is_the_tailwind_color() {
    let doc = Document::new().unwrap();
    doc.set_class_name(doc.body(), "bg-red-500").unwrap();
    let frame = frame(&doc, 40.0, 40.0);
    assert!(
        near(pixel(&frame, 20, 20), [0xef, 0x44, 0x44]),
        "{:?}",
        pixel(&frame, 20, 20)
    );
}

#[test]
fn flex_row_splits_the_viewport() {
    let doc = Document::new().unwrap();
    doc.set_class_name(doc.body(), "flex flex-row").unwrap();
    let left = doc.create_element("div").unwrap();
    let right = doc.create_element("div").unwrap();
    doc.set_class_name(left, "w-1/2 h-10 bg-red-500").unwrap();
    doc.set_class_name(right, "w-1/2 h-10 bg-blue-500").unwrap();
    doc.append_child(doc.body(), left).unwrap();
    doc.append_child(doc.body(), right).unwrap();
    let frame = frame(&doc, 200.0, 80.0);
    let left_box = frame.boxes.iter().find(|b| b.id == left).unwrap();
    let right_box = frame.boxes.iter().find(|b| b.id == right).unwrap();
    assert!(
        (left_box.width - 100.0).abs() < 1.5,
        "left {}",
        left_box.width
    );
    assert!((right_box.x - 100.0).abs() < 1.5, "right x {}", right_box.x);
    assert_eq!(frame.hit_test(20.0, 10.0), Some(left));
    assert_eq!(frame.hit_test(150.0, 10.0), Some(right));
}

#[test]
fn gap_and_padding_offset_children() {
    let doc = Document::new().unwrap();
    doc.set_class_name(doc.body(), "flex flex-row gap-4 p-4")
        .unwrap();
    let first = doc.create_element("div").unwrap();
    let second = doc.create_element("div").unwrap();
    doc.set_class_name(first, "w-10 h-10 bg-black").unwrap();
    doc.set_class_name(second, "w-10 h-10 bg-black").unwrap();
    doc.append_child(doc.body(), first).unwrap();
    doc.append_child(doc.body(), second).unwrap();
    let frame = frame(&doc, 200.0, 80.0);
    let first_box = frame.boxes.iter().find(|b| b.id == first).unwrap();
    let second_box = frame.boxes.iter().find(|b| b.id == second).unwrap();
    assert!((first_box.x - 16.0).abs() < 1.5, "x {}", first_box.x);
    assert!((first_box.y - 16.0).abs() < 1.5, "y {}", first_box.y);
    assert!(
        (second_box.x - (16.0 + 40.0 + 16.0)).abs() < 1.5,
        "second {}",
        second_box.x
    );
}

#[test]
fn grid_has_two_equal_columns() {
    let doc = Document::new().unwrap();
    doc.set_class_name(doc.body(), "grid grid-cols-2").unwrap();
    let left = doc.create_element("div").unwrap();
    let right = doc.create_element("div").unwrap();
    doc.set_class_name(left, "h-8 bg-emerald-500").unwrap();
    doc.set_class_name(right, "h-8 bg-emerald-700").unwrap();
    doc.append_child(doc.body(), left).unwrap();
    doc.append_child(doc.body(), right).unwrap();
    let frame = frame(&doc, 200.0, 80.0);
    let left_box = frame.boxes.iter().find(|b| b.id == left).unwrap();
    let right_box = frame.boxes.iter().find(|b| b.id == right).unwrap();
    assert!((left_box.width - 100.0).abs() < 2.0, "{}", left_box.width);
    assert!((right_box.x - 100.0).abs() < 2.0, "{}", right_box.x);
}

#[test]
fn hover_and_important_and_breakpoints() {
    let doc = Document::new().unwrap();
    doc.set_class_name(doc.body(), "bg-red-500 hover:bg-blue-500")
        .unwrap();
    let plain = frame(&doc, 20.0, 20.0);
    assert!(near(pixel(&plain, 10, 10), [0xef, 0x44, 0x44]));
    let hovered = doc
        .render(&RenderOptions {
            width: 20.0,
            height: 20.0,
            scale: 1.0,
            hover: Some(doc.body()),
            focus: None,
        })
        .unwrap();
    assert!(
        near(pixel(&hovered, 10, 10), [0x3b, 0x82, 0xf6]),
        "{:?}",
        pixel(&hovered, 10, 10)
    );

    doc.set_class_name(doc.body(), "!bg-red-500 bg-blue-500")
        .unwrap();
    let important = frame(&doc, 20.0, 20.0);
    assert!(near(pixel(&important, 10, 10), [0xef, 0x44, 0x44]));

    doc.set_class_name(doc.body(), "bg-red-500 md:bg-blue-500")
        .unwrap();
    let narrow = frame(&doc, 500.0, 20.0);
    let wide = frame(&doc, 800.0, 20.0);
    assert!(near(pixel(&narrow, 10, 10), [0xef, 0x44, 0x44]));
    assert!(near(pixel(&wide, 10, 10), [0x3b, 0x82, 0xf6]));
}

#[test]
fn dark_variant_follows_the_document_flag() {
    let doc = Document::new().unwrap();
    doc.set_class_name(doc.body(), "bg-white dark:bg-black")
        .unwrap();
    assert!(near(pixel(&frame(&doc, 10.0, 10.0), 5, 5), [255, 255, 255]));
    doc.set_dark(true);
    assert!(near(pixel(&frame(&doc, 10.0, 10.0), 5, 5), [0, 0, 0]));
}

#[test]
fn text_paints_glyphs() {
    let doc = Document::new().unwrap();
    doc.set_class_name(doc.body(), "bg-black text-white text-2xl")
        .unwrap();
    doc.set_text_content(doc.body(), "Hello").unwrap();
    let frame = frame(&doc, 180.0, 80.0);
    let ink = frame
        .rgba
        .chunks(4)
        .any(|px| px[0] > 200 && px[1] > 200 && px[2] > 200);
    assert!(ink, "expected light glyphs on the black background");
}

#[test]
fn a_second_frame_matches_the_first() {
    let doc = Document::new().unwrap();
    doc.set_class_name(doc.body(), "bg-red-500 text-white text-xl")
        .unwrap();
    doc.set_text_content(doc.body(), "Hello").unwrap();
    let options = RenderOptions {
        width: 120.0,
        height: 40.0,
        scale: 2.0,
        hover: None,
        focus: None,
    };
    let first = doc.render(&options).unwrap();
    let second = doc.render(&options).unwrap();
    assert_eq!(first.rgba, second.rgba);
    let (width, height) = sweet_core::physical_size(&options);
    assert_eq!((width, height), (first.width, first.height));
    let mut packed = vec![0u32; (width * height) as usize];
    sweet_core::render_xrgb(&doc, &options, &mut packed).unwrap();
    for (index, pixel) in first.rgba.chunks(4).enumerate() {
        let expected = ((pixel[0] as u32) << 16) | ((pixel[1] as u32) << 8) | pixel[2] as u32;
        assert_eq!(packed[index], expected, "pixel {index}");
    }
}

#[test]
fn style_attribute_is_rejected() {
    let doc = Document::new().unwrap();
    let err = doc
        .set_attribute(doc.body(), "style", "color: red")
        .unwrap_err();
    assert!(err.to_string().contains("no CSS"));
}

#[test]
fn negative_margin_pulls_the_box() {
    let doc = Document::new().unwrap();
    let child = doc.create_element("div").unwrap();
    doc.set_class_name(child, "-ml-4 w-10 h-10 bg-black")
        .unwrap();
    doc.append_child(doc.body(), child).unwrap();
    let frame = frame(&doc, 100.0, 40.0);
    let child_box = frame.boxes.iter().find(|b| b.id == child).unwrap();
    assert!((child_box.x + 16.0).abs() < 1.5, "{}", child_box.x);
}

#[test]
fn overflow_hidden_clips_the_child() {
    let doc = Document::new().unwrap();
    let clip = doc.create_element("div").unwrap();
    let child = doc.create_element("div").unwrap();
    doc.set_class_name(clip, "overflow-hidden w-10 h-10")
        .unwrap();
    doc.set_class_name(child, "w-20 h-10 bg-red-500").unwrap();
    doc.append_child(doc.body(), clip).unwrap();
    doc.append_child(clip, child).unwrap();
    let frame = frame(&doc, 80.0, 40.0);
    assert!(
        near(pixel(&frame, 10, 10), [0xef, 0x44, 0x44]),
        "{:?}",
        pixel(&frame, 10, 10)
    );
    assert!(
        near(pixel(&frame, 50, 10), [255, 255, 255]),
        "{:?}",
        pixel(&frame, 50, 10)
    );
}
