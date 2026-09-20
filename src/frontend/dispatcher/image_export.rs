use crate::frontend::{
    state::{AppState, ImageExportPrompt},
    viewport::PendingViewportPngExport,
};

pub(super) fn open(state: &mut AppState) {
    state.ui.pending_image_export = Some(ImageExportPrompt::new(state.ui.scripted_viewport_size));
}

pub(super) fn choose_path(state: &mut AppState) {
    let Some(prompt) = state.ui.pending_image_export.as_mut() else {
        return;
    };
    let path = std::path::Path::new(&prompt.path);
    let mut dialog = rfd::FileDialog::new().add_filter("PNG image", &["png"]);
    if let Some(name) = path.file_name() {
        dialog = dialog.set_file_name(name.to_string_lossy());
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        dialog = dialog.set_directory(parent);
    }
    prompt.apply_chosen_path(dialog.save_file());
}

pub(super) fn run(state: &mut AppState) {
    submit(state, |path| {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Overwrite existing file?")
            .set_description(format!(
                "{} already exists and will be replaced.",
                path.display()
            ))
            .set_buttons(rfd::MessageButtons::OkCancel)
            .show()
            == rfd::MessageDialogResult::Ok
    });
}

fn submit(state: &mut AppState, confirm: impl FnOnce(&std::path::Path) -> bool) {
    let Some(prompt) = state.ui.pending_image_export.as_ref() else {
        return;
    };
    let (output_path, [width, height]) = match prompt.validate() {
        Ok(values) => values,
        Err(error) => {
            state.status_error(format!("Image export: {error}"));
            return;
        }
    };
    if output_path.exists() && !confirm(&output_path) {
        return;
    }
    let request = PendingViewportPngExport {
        background: prompt.background,
        structure: state.structure().clone(),
        camera: state.ui.camera,
        selection: state.ui.selection.clone(),
        visual_state: state.ui.viewport.clone(),
        width,
        height,
        output_path,
    };
    state.ui.pending_viewport_exports.push_back(request);
    state.ui.pending_image_export = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{
        actions::AppAction, dispatcher::dispatch, viewport::ImageExportBackground,
    };
    use eframe::egui::{Color32, Context};

    #[test]
    fn image_export_draft_cancel_and_picker_cancel_preserve_view() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let ctx = Context::default();
        state.ui.scripted_viewport_size = [640, 480];
        let visual = state.ui.viewport.clone();
        let camera = state.ui.camera;
        dispatch(&mut state, AppAction::OpenImageExportDialog, &ctx);
        let prompt = state.ui.pending_image_export.as_mut().unwrap();
        assert_eq!(prompt.width, "640");
        assert_eq!(prompt.height, "480");
        assert_eq!(prompt.background, ImageExportBackground::Viewport);
        prompt.width = "1200".into();
        prompt.path = "kept.png".into();
        prompt.background = ImageExportBackground::Transparent;
        prompt.apply_chosen_path(None);
        assert_eq!(prompt.path, "kept.png");
        dispatch(&mut state, AppAction::CancelImageExport, &ctx);
        assert!(state.ui.pending_image_export.is_none());
        assert!(state.ui.pending_viewport_exports.is_empty());
        assert_eq!(state.ui.scripted_viewport_size, [640, 480]);
        assert_eq!(state.ui.viewport.background_color, visual.background_color);
        assert_eq!(state.ui.camera.yaw, camera.yaw);
    }

    #[test]
    fn image_export_dispatch_snapshots_at_submit_for_all_backgrounds() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let ctx = Context::default();
        let dir =
            std::env::temp_dir().join(format!("silicolab-image-export-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for background in [
            ImageExportBackground::Viewport,
            ImageExportBackground::White,
            ImageExportBackground::Transparent,
            ImageExportBackground::Custom(Color32::RED),
        ] {
            dispatch(&mut state, AppAction::OpenImageExportDialog, &ctx);
            let path = dir.join("image.png");
            let prompt = state.ui.pending_image_export.as_mut().unwrap();
            prompt.path = path.to_string_lossy().into_owned();
            prompt.width = "123".into();
            prompt.height = "456".into();
            prompt.background = background;
            state.structure_mut().title = "at submit".into();
            state.ui.camera.yaw = 0.75;
            state.ui.viewport.background_color = Some(Color32::BLUE);
            dispatch(&mut state, AppAction::RunImageExport, &ctx);
            dispatch(&mut state, AppAction::RunImageExport, &ctx);
            assert_eq!(state.ui.pending_viewport_exports.len(), 1);
            state.structure_mut().title = "later".into();
            state.ui.selection.clear();
            state.ui.camera.yaw = 1.5;
            state.ui.viewport.background_color = None;
            let request = state.ui.pending_viewport_exports.pop_front().unwrap();
            assert_eq!(request.background, background);
            assert_eq!(request.output_path, path);
            assert_eq!([request.width, request.height], [123, 456]);
            assert_eq!(request.structure.title, "at submit");
            assert!(request.structure.atoms.is_empty());
            assert_eq!(request.camera.yaw, 0.75);
            assert_eq!(request.visual_state.background_color, Some(Color32::BLUE));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn image_export_retains_selected_structure_after_entry_switch() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let ctx = Context::default();
        let structure = crate::domain::Structure::new(
            "carbon",
            vec![crate::domain::Atom {
                element: "C".into(),
                position: nalgebra::Point3::origin(),
                charge: 0.0,
            }],
        );
        state
            .entries
            .add_entry(structure, None, "carbon.xyz".into());
        dispatch(&mut state, AppAction::OpenImageExportDialog, &ctx);
        state.ui.pending_image_export.as_mut().unwrap().path = std::env::temp_dir()
            .join(format!("{}.png", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned();
        state.ui.selection.select_only(0);
        dispatch(&mut state, AppAction::RunImageExport, &ctx);
        state
            .entries
            .add_entry(crate::domain::Structure::empty(), None, "empty.xyz".into());
        state.ui.selection.clear();
        let request = state.ui.pending_viewport_exports.front().unwrap();
        assert_eq!(request.structure.title, "carbon");
        assert_eq!(request.structure.atoms[0].element, "C");
        assert_eq!(request.selection.ordered_indices(), vec![0]);
        assert_eq!(request.selection.primary(), Some(0));
        assert!(state.structure().atoms.is_empty());
    }

    #[test]
    fn image_export_invalid_input_keeps_draft_and_does_not_queue() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        for (path, width, height) in [
            ("", "1", "1"),
            ("x.jpg", "1", "1"),
            ("x.png", "0", "1"),
            ("x.png", "-1", "1"),
            ("x.png", "1", "1.5"),
            ("x.png", "4294967296", "1"),
        ] {
            open(&mut state);
            let prompt = state.ui.pending_image_export.as_mut().unwrap();
            prompt.path = path.into();
            prompt.width = width.into();
            prompt.height = height.into();
            submit(&mut state, |_| {
                panic!("invalid input must not prompt for overwrite")
            });
            assert!(state.ui.pending_image_export.is_some());
            assert!(state.ui.pending_viewport_exports.is_empty());
        }
    }

    #[test]
    fn image_export_overwrite_requires_confirmation() {
        let dir =
            std::env::temp_dir().join(format!("silicolab-image-export-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("existing.png");
        std::fs::write(&path, b"original").unwrap();
        let mut state = AppState::scratch(Default::default(), Vec::new());
        open(&mut state);
        state.ui.pending_image_export.as_mut().unwrap().path = path.to_string_lossy().into_owned();
        submit(&mut state, |target| {
            assert_eq!(target, path);
            false
        });
        assert!(state.ui.pending_image_export.is_some());
        assert!(state.ui.pending_viewport_exports.is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        submit(&mut state, |_| true);
        assert_eq!(state.ui.pending_viewport_exports.len(), 1);
        assert!(state.ui.pending_image_export.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
