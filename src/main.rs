mod brew;
mod brew_view;
mod settings;
mod settings_view;
mod window_state;

use gpui_kit::component::{ActiveTheme, Theme, WindowExt};
use gpui_kit::*;
use settings::Settings;
use window_state::WindowState;

gpui_kit::assets::icon_assets!(
    SettingsIcons,
    [
        Upload,
        Download,
        Search,
        AppWindow,
        Terminal,
        ArrowUp,
        ArrowRight,
        Trash,
        RefreshCw,
        Activity,
        EllipsisVertical,
        Info,
        ExternalLink
    ]
);

struct Assets;

fn configure_button_feedback(cx: &mut App) {
    Theme::update(cx, |theme| {
        theme.colors.button_primary_hover = Hsla {
            l: (theme.primary.l + 0.08).min(1.0),
            ..theme.primary
        };
    });
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if path == "icons/app.png" {
            return Ok(Some(std::borrow::Cow::Borrowed(include_bytes!(
                "../assets/app-icon.png"
            ))));
        }
        if let Some(bytes) = SettingsIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(SettingsIcons.list(path)?);
        if "icons/app.png".starts_with(path) {
            paths.push("icons/app.png".into());
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

impl Global for Settings {}

actions!(base_app, [Quit, OpenSettings]);

struct AppView {
    brew: Entity<brew_view::BrewView>,
    window_path: Option<std::path::PathBuf>,
    window_state: WindowState,
}

impl AppView {
    fn new(
        window_path: Option<std::path::PathBuf>,
        restored: Option<WindowState>,
        brew_path: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut window_state = restored.unwrap_or_else(|| WindowState::capture(window));
        window_state.update(window);
        cx.observe_window_bounds(window, |view, window, _| view.window_state.update(window))
            .detach();
        cx.on_app_quit(|view, _| {
            view.save_window_state();
            async {}
        })
        .detach();
        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |view, cx| {
                if !view.can_close(window, cx) {
                    return false;
                }
                view.window_state.update(window);
                view.save_window_state();
                true
            })
            .unwrap_or(true)
        });
        Self {
            brew: cx.new(|cx| brew_view::BrewView::new(brew_path, window, cx)),
            window_path,
            window_state,
        }
    }

    fn can_close(&self, window: &mut Window, cx: &mut App) -> bool {
        if self.brew.read(cx).busy {
            window.open_alert_dialog(cx, |dialog, _, _| {
                dialog
                    .title("Homebrew is still working")
                    .description("Wait for the current command to finish before closing the app.")
            });
            return false;
        }
        true
    }

    fn save_window_state(&self) {
        if let Some(path) = &self.window_path
            && let Err(error) = settings::write_json(path, &self.window_state)
        {
            eprintln!(
                "Couldn’t save window geometry to {}: {error}",
                path.display()
            );
        }
    }
}

impl Render for AppView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(cx.theme().background)
            .child(self.brew.clone())
    }
}

fn main() {
    let loaded = Settings::path().and_then(|path| {
        Settings::load(&path)
            .map(|settings| (path.clone(), settings))
            .map_err(|error| {
                std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))
            })
    });
    let (settings_path, settings, startup_error) = match loaded {
        Ok((path, settings)) => (Some(path), settings, None),
        Err(error) => (
            None,
            Settings::default(),
            Some(format!(
                "Couldn’t load settings: {error}. Existing files are preserved and saving is disabled. Fix the file or folder permissions, then restart the app."
            )),
        ),
    };
    let (window_path, restored) = match WindowState::load() {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!(
                "Couldn’t load window geometry: {error}. Existing file is preserved; geometry saving is disabled."
            );
            (None, None)
        }
    };
    let window_handle = std::rc::Rc::new(std::cell::Cell::new(None::<AnyWindowHandle>));
    let application = gpui_kit::application().with_assets(Assets);
    application.on_reopen({
        let window_handle = window_handle.clone();
        move |cx| {
            if let Some(handle) = window_handle.get() {
                cx.defer(move |cx| {
                    if let Err(error) = cx.update_window(handle, |_, window, cx| {
                        window.activate_window();
                        cx.activate(true);
                    }) {
                        eprintln!("Couldn’t reactivate the app window: {error}");
                    }
                });
            }
        }
    });
    application.run(move |cx| {
        gpui_kit::init(cx);
        cx.set_global(settings);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let options = WindowOptions {
            window_bounds: Some(WindowState::window_bounds(restored.as_ref(), cx)),
            titlebar: Some(TitlebarOptions {
                title: Some("Homebrew".into()),
                ..Default::default()
            }),
            window_min_size: Some(size(px(900.), px(560.))),
            ..Default::default()
        };
        let (handle, view) = gpui_kit::open_window(options, cx, |window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
            configure_button_feedback(cx);
            window
                .observe_window_appearance(|window, cx| {
                    Theme::sync_system_appearance(Some(window), cx);
                    configure_button_feedback(cx);
                })
                .detach();
            cx.new(|cx| AppView::new(window_path, restored, brew::find_brew(), window, cx))
        })
        .expect("Failed to open app window");
        window_handle.set(Some(handle));
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-,", OpenSettings, None),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-q", Quit, None),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-,", OpenSettings, None),
        ]);
        cx.on_action(move |_: &Quit, cx| {
            let view = view.clone();
            cx.defer(move |cx| {
                if cx
                    .update_window(handle, |_, window, cx| {
                        view.update(cx, |view, cx| view.can_close(window, cx))
                    })
                    .unwrap_or(false)
                {
                    cx.quit();
                }
            });
        });
        settings_view::register_action(handle, settings_path, startup_error.clone(), cx);
        cx.set_menus(vec![Menu {
            name: env!("CARGO_PKG_NAME").into(),
            disabled: false,
            items: vec![
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Quit", Quit),
            ],
        }]);
        if let Some(error) = startup_error {
            let _ = cx.update_window(handle, |_, window, cx| {
                settings_view::show_error(error, window, cx);
            });
        }
        cx.activate(true);
    });
}
