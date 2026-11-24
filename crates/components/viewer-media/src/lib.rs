//! Minimalist white media viewer plugin triggered explicitly via `?view=viewer`.

use files_api::shell::{
    format_file_size, format_utc_datetime, html_escape, parent_dir_href,
    percent_encode_path, render_viewer_topbar, render_white_document,
};
use files_api::{
    FileCategory, FileViewer, ViewMode, ViewerInput, ViewerRegistryService,
};
use frame::{BoxFuture, ConfigValue, Context, Deps, FrameError, Plugin};
use http_api::{HttpResponse, StatusCode};
use std::sync::Arc;

struct MediaViewer;

impl FileViewer for MediaViewer {
    fn name(&self) -> &str {
        "viewer-media"
    }

    fn priority(&self) -> i32 {
        20
    }

    fn supports(&self, category: FileCategory, mode: ViewMode) -> bool {
        mode == ViewMode::Viewer
            && matches!(
                category,
                FileCategory::Image | FileCategory::Video | FileCategory::Audio
            )
    }

    fn render(&self, input: ViewerInput<'_>) -> Option<HttpResponse> {
        let encoded = format!(
            "/{}",
            percent_encode_path(input.rel_path.trim_start_matches('/'))
        );
        let parent = parent_dir_href(input.rel_path);
        let meta = format!(
            "{} · {} · {}",
            format_file_size(input.size),
            input.mime,
            format_utc_datetime(input.modified)
        );
        let actions = format!(
            "<a class=\"active\" href=\"{p}?view=viewer\">Viewer</a>\
             <a href=\"{p}\">Raw</a>\
             <a href=\"{p}?view=download\">Download</a>",
            p = html_escape(&encoded),
        );
        let topbar =
            render_viewer_topbar(&parent, input.file_name, &meta, &actions);

        let src = html_escape(&encoded);
        let name_esc = html_escape(input.file_name);
        let element = match input.category {
            FileCategory::Image => {
                format!("<img src=\"{src}\" alt=\"{name_esc}\">")
            }
            FileCategory::Video => format!(
                "<video controls preload=\"metadata\" src=\"{src}\"></video>"
            ),
            FileCategory::Audio => format!(
                "<audio controls preload=\"metadata\" src=\"{src}\"></audio>"
            ),
            _ => return None,
        };

        let body = format!(
            "{topbar}\n<section class=\"d-media-frame\">{element}</section>"
        );
        Some(HttpResponse::html(
            StatusCode::OK,
            render_white_document(input.file_name, "", &body),
        ))
    }
}

/// Plugin registering [`MediaViewer`] into [`ViewerRegistryService`].
#[derive(Debug, Default, Clone, Copy)]
pub struct MediaViewerPlugin;

impl Plugin for MediaViewerPlugin {
    fn name(&self) -> &str {
        "viewer-media"
    }

    fn deps(&self) -> Deps {
        Deps::empty().with::<ViewerRegistryService>()
    }

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        Box::pin(async move {
            let registry = ctx.require::<ViewerRegistryService>()?;
            let handle = registry.register(Arc::new(MediaViewer))?;
            let _ = ctx.effect_sync("viewer-media-mount", move |scope| {
                scope.defer_async(move || handle.dispose());
                Ok(())
            })?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn renders_image_video_audio_viewer_pages() {
        for (cat, path, mime, tag) in [
            (
                FileCategory::Image,
                "/a.svg",
                "image/svg+xml",
                "<img src=\"/a.svg\"",
            ),
            (
                FileCategory::Video,
                "/b.mp4",
                "video/mp4",
                "<video controls",
            ),
            (
                FileCategory::Audio,
                "/c.mp3",
                "audio/mpeg",
                "<audio controls",
            ),
        ] {
            let resp = MediaViewer
                .render(ViewerInput {
                    rel_path: path,
                    file_name: &path[1..],
                    full_path: Path::new(&path[1..]),
                    size: 100,
                    modified: None,
                    category: cat,
                    view_mode: ViewMode::Viewer,
                    mime,
                    allow_hidden: false,
                    text: None,
                })
                .expect("media rendered");
            if let http_api::BodyKind::Bytes(b) = resp.body {
                let html = String::from_utf8(b).unwrap();
                assert!(html.contains(tag), "{html}");
                assert!(html.contains("#ffffff"));
                assert!(!html.contains("#0d1117"));
            } else {
                panic!("expected bytes body");
            }
        }
    }
}
