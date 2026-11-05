//! Cost of one poll plus thirty variable reads on a 32 KiB row.

use criterion::{Criterion, criterion_group, criterion_main};
use irsdk::defs::VarType;
use irsdk::sampler::Sampler;
use irsdk::testing::{LayoutBuilder, RowBuilder, live_image};
use irsdk::vars::Var;
use irsdk::view::SliceView;

fn bench_poll(c: &mut Criterion) {
    let mut b = LayoutBuilder::new().tick_rate(60).num_buf(3);
    for i in 0..30 {
        b = b.var(&format!("Value{i}"), VarType::Float, 1, "", "");
    }
    // Pad the row to about 32 KiB.
    let layout = b.var("Pad", VarType::Float, 8000, "", "").build();
    let row = RowBuilder::new(&layout).into_bytes();
    let image = live_image(&layout, "---\n", &[(1, row)]);
    let mut sampler = Sampler::attach(SliceView::new(&image)).expect("attach");
    let handles: Vec<Var<f32>> = (0..30)
        .map(|i| sampler.vars().find(&format!("Value{i}")).expect("var"))
        .collect();
    assert!(layout.buf_len() >= 32 * 1000);

    c.bench_function("poll and 30 reads, no new row", |bench| {
        bench.iter(|| {
            let poll = sampler.poll();
            let mut sum = 0.0f32;
            for h in &handles {
                sum += h.read(sampler.row());
            }
            std::hint::black_box((poll, sum))
        })
    });
}

criterion_group!(benches, bench_poll);
criterion_main!(benches);
