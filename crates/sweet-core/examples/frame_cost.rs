use std::time::Instant;
use sweet_core::{render_argb, render_xrgb, Document, RenderOptions};
fn main() {
    let doc = Document::new().unwrap();
    let stage = doc.create_element("div").unwrap();
    doc.set_class_name(stage, "absolute inset-0 perspective-[1600px]")
        .unwrap();
    doc.append_child(doc.body(), stage).unwrap();
    let mut video = None;
    for i in 0..5 {
        let card = doc.create_element("div").unwrap();
        doc.set_class_name(card, &format!("absolute top-[150px] left-[{}px] w-[400px] h-[250px] bg-blue-600 animate-float [transform:rotateY(35deg)]", 100+i*150)).unwrap();
        doc.append_child(stage, card).unwrap();
        if i == 2 {
            let node = doc.create_element("video").unwrap();
            doc.set_class_name(node, "absolute inset-0 w-full h-full object-cover")
                .unwrap();
            doc.append_child(card, node).unwrap();
            video = Some(node);
        }
    }
    let mut buffer = vec![0; 1440 * 900];
    for streaming in [false, true] {
        for rounded in [false, true] {
            doc.set_class_name(
                doc.body(),
                if rounded {
                    "bg-black overflow-hidden rounded-2xl border border-white/10"
                } else {
                    "bg-black overflow-hidden"
                },
            )
            .unwrap();
            for argb in [false, true] {
                let mut times = Vec::new();
                for i in 0..130 {
                    let options = RenderOptions {
                        width: 1440.0,
                        height: 900.0,
                        time: i as f32 / 60.0,
                        ..Default::default()
                    };
                    if streaming && i % 2 == 0 {
                        doc.set_image_rgba(
                            video.unwrap(),
                            640,
                            360,
                            [i as u8, 100, 180, 255].repeat(640 * 360),
                        )
                        .unwrap();
                    }
                    let start = Instant::now();
                    if argb {
                        render_argb(&doc, &options, 1.0, &mut buffer).unwrap();
                    } else {
                        render_xrgb(&doc, &options, &mut buffer).unwrap();
                    }
                    if i >= 10 {
                        times.push(start.elapsed().as_secs_f64() * 1000.0);
                    }
                }
                times.sort_by(f64::total_cmp);
                println!(
                "video={streaming} rounded={rounded} argb={argb}: median {:.2}ms p95 {:.2}ms max {:.2}ms over-budget {}/120",
                times[60], times[114], times[119], times.iter().filter(|ms| **ms > 1000.0 / 60.0).count()
            );
            }
        }
    }
}
