use sweet_core::{Document, RenderOptions};

fn frame(doc: &Document, width: f32, height: f32) -> sweet_core::Frame {
    frame_at(doc, width, height, 0.0)
}

fn frame_at(doc: &Document, width: f32, height: f32, time: f32) -> sweet_core::Frame {
    doc.render(&RenderOptions {
        width,
        height,
        scale: 1.0,
        hover: None,
        focus: None,
        time,
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
            time: 0.0,
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
            time: 0.0,
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
        time: 0.0,
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

const RED: [u8; 3] = [239, 68, 68];

#[test]
fn arbitrary_translate_moves_the_box() {
    let doc = Document::new().unwrap();
    let card = doc.create_element("div").unwrap();
    doc.set_class_name(
        card,
        "absolute left-[20px] top-[20px] h-[30px] w-[30px] bg-red-500 [transform:translateX(40px)]",
    )
    .unwrap();
    doc.append_child(doc.body(), card).unwrap();
    let frame = frame(&doc, 160.0, 80.0);
    assert!(
        near(pixel(&frame, 70, 35), RED),
        "translated {:?}",
        pixel(&frame, 70, 35)
    );
    assert!(
        !near(pixel(&frame, 30, 35), RED),
        "unshifted {:?}",
        pixel(&frame, 30, 35)
    );
}

#[test]
fn perspective_brings_translate_z_forward() {
    let doc = Document::new().unwrap();
    let stage = doc.create_element("div").unwrap();
    doc.set_class_name(
        stage,
        "absolute left-[100px] top-[100px] h-[100px] w-[100px] perspective-[200px]",
    )
    .unwrap();
    let card = doc.create_element("div").unwrap();
    doc.set_class_name(
        card,
        "absolute inset-0 bg-red-500 [transform:translateZ(50px)]",
    )
    .unwrap();
    doc.append_child(stage, card).unwrap();
    doc.append_child(doc.body(), stage).unwrap();
    let frame = frame(&doc, 400.0, 400.0);
    assert!(
        near(pixel(&frame, 150, 150), RED),
        "center {:?}",
        pixel(&frame, 150, 150)
    );
    // Untransformed top is y=100. d/(d-z) = 200/150 pulls the edge out to about y=83.
    assert!(
        near(pixel(&frame, 150, 90), RED),
        "forward {:?}",
        pixel(&frame, 150, 90)
    );
    assert!(
        !near(pixel(&frame, 150, 70), RED),
        "outside {:?}",
        pixel(&frame, 150, 70)
    );
}

#[test]
fn bracket_z_index_paints_above() {
    let doc = Document::new().unwrap();
    let high = doc.create_element("div").unwrap();
    doc.set_class_name(
        high,
        "absolute left-[10px] top-[10px] h-[20px] w-[20px] bg-red-500 z-[28]",
    )
    .unwrap();
    let low = doc.create_element("div").unwrap();
    doc.set_class_name(
        low,
        "absolute left-[10px] top-[10px] h-[20px] w-[20px] bg-blue-500 z-10",
    )
    .unwrap();
    doc.append_child(doc.body(), high).unwrap();
    doc.append_child(doc.body(), low).unwrap();
    let frame = frame(&doc, 40.0, 40.0);
    assert!(
        near(pixel(&frame, 20, 20), RED),
        "z {:?}",
        pixel(&frame, 20, 20)
    );
}

#[test]
fn stage_width_is_the_narrower_of_vw_and_rem() {
    let doc = Document::new().unwrap();
    let stage = doc.create_element("div").unwrap();
    doc.set_class_name(stage, "w-[min(52vw,22rem)] aspect-video bg-red-500")
        .unwrap();
    doc.append_child(doc.body(), stage).unwrap();
    let frame = frame(&doc, 960.0, 400.0);
    let laid = frame.boxes.iter().find(|b| b.id == stage).unwrap();
    assert!((laid.width - 352.0).abs() < 1.0, "width {}", laid.width);
    assert!((laid.height - 198.0).abs() < 1.5, "height {}", laid.height);
}

#[test]
fn spin_reuses_the_layer_and_still_turns() {
    let doc = Document::new().unwrap();
    let bar = doc.create_element("div").unwrap();
    doc.set_class_name(
        bar,
        "absolute left-[40px] top-[40px] h-[20px] w-[80px] bg-red-500 animate-spin",
    )
    .unwrap();
    doc.append_child(doc.body(), bar).unwrap();
    let flat = frame_at(&doc, 160.0, 160.0, 0.0);
    assert!(flat.rasterized_layers >= 1, "first frame built no layer");
    assert!(flat.animating);
    assert!(
        near(pixel(&flat, 110, 50), RED),
        "flat {:?}",
        pixel(&flat, 110, 50)
    );
    let turned = frame_at(&doc, 160.0, 160.0, 0.25);
    assert_eq!(turned.rasterized_layers, 0, "spin rebuilt the layer");
    assert!(
        !near(pixel(&turned, 110, 50), RED),
        "still flat {:?}",
        pixel(&turned, 110, 50)
    );
    assert!(
        near(pixel(&turned, 80, 80), RED),
        "turned {:?}",
        pixel(&turned, 80, 80)
    );
}

#[test]
fn pose_change_reuses_the_layer() {
    let doc = Document::new().unwrap();
    let card = doc.create_element("div").unwrap();
    doc.set_class_name(
        card,
        "absolute left-[20px] top-[30px] h-[20px] w-[20px] bg-red-500 [transform:translateX(8px)_translateZ(0px)_rotateY(0deg)_scale(1)]",
    )
    .unwrap();
    doc.append_child(doc.body(), card).unwrap();
    let first = frame(&doc, 200.0, 80.0);
    assert!(first.rasterized_layers >= 1);
    doc.set_class_name(
        card,
        "absolute left-[20px] top-[30px] h-[20px] w-[20px] bg-red-500 [transform:translateX(80px)_translateZ(0px)_rotateY(0deg)_scale(1)]",
    )
    .unwrap();
    let second = frame(&doc, 200.0, 80.0);
    assert_eq!(second.rasterized_layers, 0);
    assert!(
        near(pixel(&second, 110, 40), RED),
        "moved {:?}",
        pixel(&second, 110, 40)
    );
    assert!(
        !near(pixel(&second, 30, 40), RED),
        "old {:?}",
        pixel(&second, 30, 40)
    );
}

#[test]
fn ease_out_transition_is_between_the_poses() {
    let doc = Document::new().unwrap();
    let card = doc.create_element("div").unwrap();
    let base = "absolute left-[20px] top-[30px] h-[16px] w-[16px] bg-red-500 transition-all duration-700 ease-out";
    doc.set_class_name(card, &format!("{base} [transform:translateX(0px)]"))
        .unwrap();
    doc.append_child(doc.body(), card).unwrap();
    let start = frame_at(&doc, 220.0, 80.0, 0.0);
    assert!(near(pixel(&start, 28, 38), RED));
    doc.set_class_name(card, &format!("{base} [transform:translateX(100px)]"))
        .unwrap();
    let begun = frame_at(&doc, 220.0, 80.0, 0.0);
    assert!(
        near(pixel(&begun, 28, 38), RED),
        "start {:?}",
        pixel(&begun, 28, 38)
    );
    let mid = frame_at(&doc, 220.0, 80.0, 0.35);
    // Halfway through ease-out is about 84% of the 100px slide, so the box sits near x=104.
    assert!(
        !near(pixel(&mid, 28, 38), RED),
        "still home {:?}",
        pixel(&mid, 28, 38)
    );
    assert!(
        near(pixel(&mid, 110, 38), RED),
        "mid {:?}",
        pixel(&mid, 110, 38)
    );
    assert!(
        !near(pixel(&mid, 78, 38), RED),
        "only linear {:?}",
        pixel(&mid, 78, 38)
    );
    assert!(
        !near(pixel(&mid, 130, 38), RED),
        "already done {:?}",
        pixel(&mid, 130, 38)
    );
    let end = frame_at(&doc, 220.0, 80.0, 0.7);
    assert!(
        near(pixel(&end, 128, 38), RED),
        "end {:?}",
        pixel(&end, 128, 38)
    );
}

#[test]
fn animation_replay_is_cheap() {
    if cfg!(debug_assertions) {
        return;
    }
    let doc = Document::new().unwrap();
    doc.set_class_name(
        doc.body(),
        "relative h-screen w-full overflow-hidden bg-[#070709] px-6 text-white",
    )
    .unwrap();
    let glow = doc.create_element("div").unwrap();
    doc.set_class_name(
        glow,
        "pointer-events-none absolute left-1/2 top-[34%] h-72 w-72 -translate-x-1/2 -translate-y-1/2 rounded-full bg-rose-400/30 blur-3xl animate-glow",
    )
    .unwrap();
    let float = doc.create_element("div").unwrap();
    doc.set_class_name(float, "animate-float").unwrap();
    let disc = doc.create_element("div").unwrap();
    doc.set_class_name(disc, "relative h-44 w-44").unwrap();
    let spin = doc.create_element("div").unwrap();
    doc.set_class_name(
        spin,
        "h-full w-full rounded-full bg-rose-500 animate-spin-slow",
    )
    .unwrap();
    let sheen_clip = doc.create_element("div").unwrap();
    doc.set_class_name(
        sheen_clip,
        "pointer-events-none absolute inset-[8%] overflow-hidden rounded-full",
    )
    .unwrap();
    let sheen = doc.create_element("div").unwrap();
    doc.set_class_name(
        sheen,
        "absolute inset-y-[-10%] w-1/3 bg-gradient-to-r from-transparent via-white/55 to-transparent animate-sheen",
    )
    .unwrap();
    let title = doc.create_element("div").unwrap();
    doc.set_class_name(title, "mt-7 text-3xl font-semibold")
        .unwrap();
    doc.set_text_content(title, "DJ Grok").unwrap();
    doc.append_child(sheen_clip, sheen).unwrap();
    doc.append_child(disc, spin).unwrap();
    doc.append_child(disc, sheen_clip).unwrap();
    doc.append_child(float, disc).unwrap();
    doc.append_child(doc.body(), glow).unwrap();
    doc.append_child(doc.body(), float).unwrap();
    doc.append_child(doc.body(), title).unwrap();
    let _ = frame_at(&doc, 960.0, 640.0, 0.0);
    let frames = 30;
    let start = std::time::Instant::now();
    for i in 1..=frames {
        let _ = frame_at(&doc, 960.0, 640.0, i as f32 / 60.0);
    }
    let per_ms = start.elapsed().as_secs_f64() * 1000.0 / frames as f64;
    eprintln!("title per frame {per_ms:.2} ms");
    assert!(
        per_ms < 8.0,
        "title animation frame took {per_ms:.2} ms, want a 60 Hz budget"
    );
}

fn cover_pose(offset: i32) -> String {
    let abs = offset.unsigned_abs() as usize;
    let shift = [0, 112, 150, 178, 198, 214][abs];
    let depth = [90, -20, -64, -104, -140, -172][abs];
    let turn = if offset < 0 {
        50
    } else if offset > 0 {
        -50
    } else {
        0
    };
    let size = [1.0, 0.98, 0.94, 0.9, 0.86, 0.82][abs];
    let x = if offset < 0 { -shift } else { shift };
    let opacity = [
        "opacity-100",
        "opacity-100",
        "opacity-95",
        "opacity-82",
        "opacity-68",
        "opacity-52",
    ][abs];
    let layer = ["z-40", "z-[28]", "z-[22]", "z-[16]", "z-[12]", "z-[8]"][abs];
    format!(
        "absolute inset-0 origin-center transition-all duration-700 ease-out {layer} {opacity} [transform:translateX({x}px)_translateZ({depth}px)_rotateY({turn}deg)_scale({size})]"
    )
}

#[test]
fn cover_transition_is_cheap() {
    if cfg!(debug_assertions) {
        return;
    }
    let doc = Document::new().unwrap();
    doc.set_class_name(
        doc.body(),
        "relative h-screen w-full overflow-hidden bg-[#070709]",
    )
    .unwrap();
    let stage_wrap = doc.create_element("div").unwrap();
    doc.set_class_name(stage_wrap, "perspective-[1600px]")
        .unwrap();
    let stage = doc.create_element("div").unwrap();
    doc.set_class_name(stage, "relative aspect-video w-[min(52vw,22rem)]")
        .unwrap();
    let mut cards = Vec::new();
    for index in 0..5 {
        let card = doc.create_element("div").unwrap();
        doc.set_class_name(card, &cover_pose(index - 2)).unwrap();
        let face = doc.create_element("div").unwrap();
        doc.set_class_name(
            face,
            "absolute inset-0 overflow-hidden rounded-xl bg-rose-500",
        )
        .unwrap();
        let reflection = doc.create_element("div").unwrap();
        doc.set_class_name(
            reflection,
            "pointer-events-none absolute inset-x-0 top-[103%] h-[42%] overflow-hidden",
        )
        .unwrap();
        let mirror = doc.create_element("div").unwrap();
        doc.set_class_name(mirror, "h-full w-full bg-rose-500 [transform:scaleY(-1)]")
            .unwrap();
        let fade = doc.create_element("div").unwrap();
        doc.set_class_name(
            fade,
            "pointer-events-none absolute inset-0 bg-gradient-to-b from-[#070709]/45 via-[#070709]/82 to-[#070709]",
        )
        .unwrap();
        doc.append_child(reflection, mirror).unwrap();
        doc.append_child(reflection, fade).unwrap();
        doc.append_child(card, face).unwrap();
        doc.append_child(card, reflection).unwrap();
        doc.append_child(stage, card).unwrap();
        cards.push(card);
    }
    doc.append_child(stage_wrap, stage).unwrap();
    doc.append_child(doc.body(), stage_wrap).unwrap();
    let _ = frame_at(&doc, 960.0, 640.0, 0.0);
    for (index, card) in cards.iter().enumerate() {
        doc.set_class_name(*card, &cover_pose(index as i32 - 3))
            .unwrap();
    }
    // The class change rebuilds the layers. Time the frames after that.
    let _ = frame_at(&doc, 960.0, 640.0, 0.0);
    let frames = 30;
    let start = std::time::Instant::now();
    for i in 1..=frames {
        let _ = frame_at(&doc, 960.0, 640.0, i as f32 / 60.0);
    }
    let per_ms = start.elapsed().as_secs_f64() * 1000.0 / frames as f64;
    eprintln!("cover per frame {per_ms:.2} ms");
    assert!(
        per_ms < 12.0,
        "cover transition frame took {per_ms:.2} ms, want to stay inside a 60 Hz frame"
    );
}

#[test]
fn rotate_y_narrows_the_cover() {
    let doc = Document::new().unwrap();
    let stage = doc.create_element("div").unwrap();
    doc.set_class_name(
        stage,
        "absolute left-[80px] top-[40px] h-[80px] w-[160px] perspective-[800px]",
    )
    .unwrap();
    let card = doc.create_element("div").unwrap();
    doc.set_class_name(
        card,
        "absolute inset-0 bg-red-500 [transform:translateX(0px)_translateZ(0px)_rotateY(50deg)_scale(1)]",
    )
    .unwrap();
    doc.append_child(stage, card).unwrap();
    doc.append_child(doc.body(), stage).unwrap();
    let frame = frame(&doc, 360.0, 180.0);
    // Unrotated the card fills x=80..240. rotateY(50) pulls both edges inward.
    assert!(
        near(pixel(&frame, 160, 80), RED),
        "center {:?}",
        pixel(&frame, 160, 80)
    );
    assert!(
        !near(pixel(&frame, 88, 80), RED),
        "left edge {:?}",
        pixel(&frame, 88, 80)
    );
    assert!(
        !near(pixel(&frame, 232, 80), RED),
        "right edge {:?}",
        pixel(&frame, 232, 80)
    );
}

#[test]
fn cover_hit_uses_the_projected_quad() {
    let doc = Document::new().unwrap();
    let stage = doc.create_element("div").unwrap();
    doc.set_class_name(
        stage,
        "absolute left-[200px] top-[80px] h-[100px] w-[100px] perspective-[400px]",
    )
    .unwrap();
    let center = doc.create_element("div").unwrap();
    doc.set_class_name(
        center,
        "absolute inset-0 bg-red-500 z-40 [transform:translateX(0px)_translateZ(40px)_rotateY(0deg)_scale(1)]",
    )
    .unwrap();
    let right = doc.create_element("div").unwrap();
    doc.set_class_name(
        right,
        "absolute inset-0 bg-blue-500 z-[28] [transform:translateX(70px)_translateZ(-20px)_rotateY(-50deg)_scale(1)]",
    )
    .unwrap();
    doc.append_child(stage, center).unwrap();
    doc.append_child(stage, right).unwrap();
    doc.append_child(doc.body(), stage).unwrap();
    let frame = frame(&doc, 520.0, 280.0);
    assert_eq!(frame.hit_test(250.0, 130.0), Some(center));
    assert_eq!(frame.hit_test(330.0, 130.0), Some(right));
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

#[test]
fn video_pixels_refresh_only_their_animated_layer() {
    let doc = Document::new().unwrap();
    let card = doc.create_element("div").unwrap();
    doc.set_class_name(card, "absolute w-[40px] h-[40px] transition-all duration-700").unwrap();
    let video = doc.create_element("video").unwrap();
    doc.set_class_name(video, "w-full h-full").unwrap();
    doc.set_image_rgba(video, 2, 2, [255, 0, 0, 255].repeat(4)).unwrap();
    doc.append_child(card, video).unwrap();
    doc.append_child(doc.body(), card).unwrap();
    let other = doc.create_element("div").unwrap();
    doc.set_class_name(other, "absolute left-[60px] w-[20px] h-[20px] bg-blue-500 animate-spin").unwrap();
    doc.append_child(doc.body(), other).unwrap();
    let first = frame_at(&doc, 100.0, 80.0, 0.0);
    assert_eq!(pixel(&first, 10, 10), [255, 0, 0, 255]);
    for i in 1..=10 {
        let color = [0, i * 20, 0, 255];
        doc.set_image_rgba(video, 2, 2, color.repeat(4)).unwrap();
        let next = frame_at(&doc, 100.0, 80.0, i as f32 / 60.0);
        assert_eq!(pixel(&next, 10, 10), color);
        assert_eq!(next.rasterized_layers, 1, "unrelated layer rebuilt");
        assert!(next.animating);
    }
    doc.clear_image(video).unwrap();
    let cleared = frame_at(&doc, 100.0, 80.0, 0.2);
    assert_ne!(pixel(&cleared, 10, 10), [0, 200, 0, 255]);
}

#[test]
fn static_image_pixels_and_intrinsic_size_still_invalidate_the_scene() {
    let doc = Document::new().unwrap();
    let image = doc.create_element("img").unwrap();
    doc.set_class_name(image, "absolute").unwrap();
    doc.append_child(doc.body(), image).unwrap();
    doc.set_image_rgba(image, 20, 20, [255, 0, 0, 255].repeat(400)).unwrap();
    let first = frame_at(&doc, 100.0, 80.0, 0.0);
    assert_eq!(pixel(&first, 10, 10), [255, 0, 0, 255]);
    doc.set_image_rgba(image, 20, 20, [0, 255, 0, 255].repeat(400)).unwrap();
    let next = frame_at(&doc, 100.0, 80.0, 0.1);
    assert_eq!(pixel(&next, 10, 10), [0, 255, 0, 255]);
    doc.set_image_rgba(image, 40, 20, [0, 0, 255, 255].repeat(800)).unwrap();
    let resized = frame_at(&doc, 100.0, 80.0, 0.2);
    assert_eq!(pixel(&resized, 30, 10), [0, 0, 255, 255]);
    assert_eq!(resized.boxes.iter().find(|b| b.id == image).unwrap().width, 40.0);
}
