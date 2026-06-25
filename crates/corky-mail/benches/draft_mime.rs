//! Benchmarks for `draft::send::build_mime_message` — the MIME composition hot path
//! behind `corky draft send`. Pure computation (no network, no GPU), so this runs
//! without the transcribe feature.

use std::path::PathBuf;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use corky_mail::draft::send::build_mime_message;

fn build_mime_benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("build_mime_message");

    let reply_to = Some("<CABx-original@mail.gmail.com>".to_string());
    group.bench_function("plain", |b| {
        b.iter(|| {
            build_mime_message(
                black_box("alice@example.com"),
                black_box("brian@example.com"),
                black_box("Test Subject"),
                black_box("Hello Alice, this is a plain-text body."),
                black_box(&None),
                black_box(&[][..]),
            )
            .unwrap()
        })
    });

    group.bench_function("in_reply_to", |b| {
        b.iter(|| {
            build_mime_message(
                black_box("alice@example.com"),
                black_box(""),
                black_box("Re: Threading a reply"),
                black_box("Body of the reply."),
                black_box(&reply_to),
                black_box(&[][..]),
            )
            .unwrap()
        })
    });

    // A 4 KiB temp payload so the base64 attachment loop does real work.
    let tmp = tempfile::NamedTempFile::new().expect("temp file");
    std::fs::write(tmp.path(), b"x".repeat(4096)).expect("write payload");
    let attachments: Vec<PathBuf> = vec![tmp.path().to_path_buf()];
    group.bench_function("with_attachment", |b| {
        b.iter(|| {
            build_mime_message(
                black_box("alice@example.com"),
                black_box("brian@example.com"),
                black_box("Subject with attachment"),
                black_box("See attached."),
                black_box(&None),
                black_box(&attachments[..]),
            )
            .unwrap()
        })
    });
    drop(tmp);

    group.finish();
}

criterion_group!(benches, build_mime_benches);
criterion_main!(benches);
