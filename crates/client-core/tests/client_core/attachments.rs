use agent_client_protocol::schema::v2 as acp;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use client_core::composer::attachment::{
    AttachmentKind, AttachmentOutcome, PromptAttachment, build_attachments, classify_attachment,
};
use client_core::session_model::WorkspaceAccess;
use std::path::Path;
use url::Url;
use utils::testing::TestWorkspace;

const ONE_MIB: usize = 1024 * 1024;
const FIVE_MIB: usize = 5 * 1024 * 1024;

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
    let media = |bytes: usize| vec![0xab; bytes];
    let multibyte = "€".repeat(ONE_MIB / 3 + 2).into_bytes();
    let mut early_invalid = text(ONE_MIB + 10);
    early_invalid[100] = 0xff;
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec();
    let flac = b"fake but valid UTF-8 audio metadata".to_vec();
    let too_large = |name: &str| format!("Skipped {name}: file too large (max 5242880)");
    let binary = |name: &str| format!("Skipped binary or non-UTF8 file: {name}");
    let truncated = |name: &str| format!("Truncated {name} to 1048576 bytes");
    let cases = [
        ("photo.png", media(FIVE_MIB), Some(("image", "image/png", FIVE_MIB)), None),
        ("photo.png", media(FIVE_MIB + 1), None, Some(too_large("photo.png"))),
        ("note.wav", media(FIVE_MIB), Some(("audio", "audio/wav", FIVE_MIB)), None),
        ("note.wav", media(FIVE_MIB + 1), None, Some(too_large("note.wav"))),
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
fn text_links_to_its_file_only_for_an_agent_that_shares_the_filesystem() {
    let workspace = TestWorkspace::new();
    let path = workspace.write("docs/My Notes.md", "# notes");
    let attachments = [PromptAttachment::new(path.clone(), workspace.root())];

    let uris = [WorkspaceAccess::Local, WorkspaceAccess::Remote].map(|access| {
        let outcome = build_attachments(&attachments, access);
        match &outcome.blocks[..] {
            [
                acp::ContentBlock::Resource(acp::EmbeddedResource {
                    resource: acp::EmbeddedResourceResource::TextResourceContents(contents),
                    ..
                }),
            ] => contents.uri.clone(),
            other => panic!("expected one text resource, got {other:?}"),
        }
    });

    let local_uri = Url::from_file_path(path.canonicalize().unwrap()).unwrap().to_string();
    assert_eq!(uris, [local_uri.as_str(), "attachment:///docs/My%20Notes.md"]);
}

#[test]
fn a_remote_agents_own_file_is_linked_for_it_to_read_instead_of_read_here() {
    let attachment = PromptAttachment::in_workspace("/srv/repo/docs/My Notes.md".into(), Path::new("/srv/repo"));

    let outcome = build_attachments(&[attachment], WorkspaceAccess::Remote);

    let link = acp::ResourceLink::new("docs/My Notes.md", "file:///srv/repo/docs/My%20Notes.md");
    assert_eq!(outcome.blocks, [acp::ContentBlock::ResourceLink(link)]);
    assert!(outcome.warnings.is_empty(), "nothing is read from this machine");
}

fn encode(files: &[(&str, impl AsRef<[u8]>)]) -> AttachmentOutcome {
    let workspace = TestWorkspace::new();
    let attachments: Vec<_> = files
        .iter()
        .map(|(name, bytes)| PromptAttachment::new(workspace.write(name, bytes), workspace.root()))
        .collect();
    build_attachments(&attachments, WorkspaceAccess::Local)
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
