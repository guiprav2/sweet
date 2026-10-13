use std::time::Instant;
use sweet_core::{render_argb, render_xrgb, Document, RenderOptions};
fn main() {
    let doc = Document::new().unwrap();
    let stage = doc.create_element("div").unwrap();
    doc.set_class_name(stage, "absolute inset-0 perspective-[1600px]")
        .unwrap();
    doc.append_child(doc.body(), stage).unwrap();
    for i in 0..5 {
        let card = doc.create_element("div").unwrap();
        doc.set_class_name(card, &format!("absolute top-[150px] left-[{}px] w-[400px] h-[250px] bg-blue-600 animate-float [transform:rotateY(35deg)]", 100+i*150)).unwrap();
        doc.append_child(stage, card).unwrap();
    }
    let mut buffer = vec![0; 1440 * 900];
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
                "rounded={rounded} argb={argb}: median {:.2}ms p95 {:.2}ms",
                times[60], times[114]
            );
        }
    }
}
