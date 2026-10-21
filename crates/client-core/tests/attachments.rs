use agent_client_protocol::schema::v2 as acp;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use client_core::composer::attachment::{
    AttachmentKind, AttachmentOutcome, PromptAttachment, build_attachments, classify_attachment,
};
use client_core::testing::FakeFilesystem;
use std::path::Path;
use utils::testing::TestWorkspace;

const ONE_MIB: usize = 1024 * 1024;
const TEN_MIB: usize = 10 * 1024 * 1024;

#[test]
fn classify_attachment_whitelists_media_and_text_types() {
    use AttachmentKind::{Audio, Image, Text, Unsupported};
    for (file, kind) in [
        ("photo.png", Image),
        ("photo.jpg", Image),
        ("photo.jpeg", Image),
        ("photo.gif", Image),
        ("photo.webp", Image),
        ("icon.svg", Unsupported),
        ("photo.bmp", Unsupported),
        ("note.wav", Audio),
        ("note.mp3", Audio),
        ("note.ogg", Audio),
        ("readme.txt", Text),
        ("data.xyz", Unsupported),
    ] {
        assert_eq!(classify_attachment(Path::new(file)), kind, "{file}");
    }
}

#[test]
fn attachments_enforce_media_and_text_size_limits() {
    let text = |bytes: usize| vec![b'x'; bytes];
    let multibyte = "€".repeat(ONE_MIB / 3 + 2).into_bytes();
    let mut early_invalid = text(ONE_MIB + 10);
    early_invalid[100] = 0xff;
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec();
    let flac = b"fake but valid UTF-8 audio metadata".to_vec();
    let too_large = |name: &str| format!("Skipped {name}: file too large (max 10485760)");
    let binary = |name: &str| format!("Skipped binary or non-UTF8 file: {name}");
    let truncated = |name: &str| format!("Truncated {name} to 1048576 bytes");
    let cases = [
        ("photo.png", vec![0; TEN_MIB], Some(("image", "image/png", TEN_MIB)), None),
        ("photo.png", vec![0; TEN_MIB + 1], None, Some(too_large("photo.png"))),
        ("note.wav", vec![0; TEN_MIB], Some(("audio", "audio/wav", TEN_MIB)), None),
        ("note.wav", vec![0; TEN_MIB + 1], None, Some(too_large("note.wav"))),
        ("notes.txt", text(ONE_MIB - 1), Some(("text", "text/plain", ONE_MIB - 1)), None),
        ("notes.txt", text(ONE_MIB), Some(("text", "text/plain", ONE_MIB)), None),
        ("notes.txt", text(ONE_MIB + 1), Some(("text", "text/plain", ONE_MIB)), Some(truncated("notes.txt"))),
        ("big.txt", text(4 * ONE_MIB), Some(("text", "text/plain", ONE_MIB)), Some(truncated("big.txt"))),
        ("multi.txt", multibyte, Some(("text", "text/plain", ONE_MIB - ONE_MIB % 3)), Some(truncated("multi.txt"))),
        ("bad.txt", early_invalid, None, Some(binary("bad.txt"))),
        ("icon.svg", svg.clone(), Some(("text", "image/svg+xml", svg.len())), None),
        ("track.flac", flac.clone(), Some(("text", "audio/flac", flac.len())), None),
        ("notes.xyz", b"hello world".to_vec(), Some(("text", "chemical/x-xyz", 11)), None),
        ("raster.bmp", vec![0x42, 0x4d, 0xff, 0x00], None, Some(binary("raster.bmp"))),
        ("data.bin", vec![0xff, 0xfe, 0xfd], None, Some(binary("data.bin"))),
    ];
    for (name, body, embedded, warning) in cases {
        let outcome = encode(&[(name, &body)]);

        let blocks: Vec<_> = outcome.blocks.iter().map(embedded_bytes).collect();
        let summary: Vec<_> = blocks.iter().map(|(kind, mime, bytes)| (*kind, *mime, bytes.len())).collect();
        assert_eq!(summary, Vec::from_iter(embedded), "{name}");
        assert!(blocks.iter().all(|(_, _, bytes)| body.starts_with(bytes)), "{name} should embed a prefix of the file");
        assert_eq!(outcome.warnings, Vec::from_iter(warning), "{name}");
    }
}

#[test]
fn multi_attachment_preserves_order_with_truncated_text_between_media() {
    let files = [
        ("photo.png", b"png bytes".to_vec()),
        ("big.txt", vec![b'a'; ONE_MIB + 5]),
        ("note.wav", b"wav bytes".to_vec()),
    ];
    let outcome = encode(&files);

    let blocks: Vec<_> =
        outcome.blocks.iter().map(embedded_bytes).map(|(kind, _, bytes)| (kind, bytes.len())).collect();
    assert_eq!(blocks, [("image", 9), ("text", ONE_MIB), ("audio", 9)]);
    assert_eq!(outcome.warnings, vec!["Truncated big.txt to 1048576 bytes"]);
}

#[test]
fn image_and_audio_blocks_carry_base64_of_the_file_bytes() {
    let image_bytes = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xde, 0xad, 0xbe, 0xef];
    let audio_bytes = *b"RIFF fake wav payload with non-text bytes \xff\x01\x02";
    let tmp = TestWorkspace::new();
    let attachments = [("photo.png", &image_bytes[..]), ("note.wav", &audio_bytes[..])]
        .map(|(name, bytes)| PromptAttachment { path: tmp.write(name, bytes), display_name: name.to_string() });

    let outcome = build_attachments(&attachments);

    let blocks: Vec<_> = outcome.blocks.iter().map(embedded_bytes).collect();
    assert_eq!(blocks, [("image", "image/png", image_bytes.to_vec()), ("audio", "audio/wav", audio_bytes.to_vec())]);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
}

fn encode(files: &[(&str, impl AsRef<[u8]>)]) -> AttachmentOutcome {
    let mut filesystem = FakeFilesystem::new();
    let mut attachments = Vec::new();
    for (name, bytes) in files {
        let path = Path::new("/workspace").join(name);
        filesystem.write_file(&path, bytes);
        attachments.push(PromptAttachment { path, display_name: (*name).to_string() });
    }
    filesystem.build_attachments(&attachments)
}

fn embedded_bytes(block: &acp::ContentBlock) -> (&'static str, &str, Vec<u8>) {
    match block {
        acp::ContentBlock::Image(image) => ("image", image.mime_type.0.as_ref(), BASE64.decode(&image.data).unwrap()),
        acp::ContentBlock::Audio(audio) => ("audio", audio.mime_type.0.as_ref(), BASE64.decode(&audio.data).unwrap()),
        acp::ContentBlock::Resource(resource) => match &resource.resource {
            acp::EmbeddedResourceResource::TextResourceContents(contents) => (
                "text",
                contents.mime_type.as_ref().map_or("", |mime| mime.0.as_ref()),
                contents.text.clone().into_bytes(),
            ),
            other => panic!("expected a text resource, got {other:?}"),
        },
        other => panic!("unexpected block {other:?}"),
    }
}
