use client_core::composer::attachment::PromptAttachment;
use gpui::{Image, ImageFormat};
use std::fs;
use utils::temp_dir::TempDir;

#[derive(Default)]
pub(crate) struct PastedImages {
    dir: TempDir,
    saved: usize,
}

impl PastedImages {
    pub(crate) fn save(&mut self, image: &Image) -> Result<PromptAttachment, String> {
        let extension = match image.format {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Gif => "gif",
            ImageFormat::Webp => "webp",
            other => {
                let format = format!("{other:?}").to_uppercase();
                return Err(format!("Can't attach {format} images; paste a PNG, JPEG, GIF or WebP instead."));
            }
        };
        self.saved += 1;
        let display_name = format!("pasted-image-{}.{extension}", self.saved);
        let path = self.dir.path().join(&display_name);
        fs::create_dir_all(self.dir.path())
            .and_then(|()| fs::write(&path, &image.bytes))
            .map_err(|error| format!("Couldn't save the pasted image: {error}"))?;
        Ok(PromptAttachment { path, display_name })
    }
}
