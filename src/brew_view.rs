use crate::brew::{self, Kind, Operation, Package};
use gpui_kit::{
    assets::IconName,
    component::{
        ActiveTheme, Disableable, Icon, Sizable, WindowExt,
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState},
        tag::Tag,
    },
    prelude::FluentBuilder,
    *,
};
use std::{path::PathBuf, time::Duration};

pub(crate) struct BrewView {
    brew: Option<PathBuf>,
    query: Entity<InputState>,
    installed: Vec<Package>,
    search_results: Vec<Package>,
    search_task: Option<Task<()>>,
    search_id: u64,
    searching: bool,
    search_error: Option<String>,
    pub(crate) busy: bool,
    status: String,
    output: String,
    failed: bool,
}

impl BrewView {
    pub(crate) fn new(brew: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search installed packages and Homebrew…")
        });
        cx.subscribe_in(&query, window, |this, _, event, _, cx| match event {
            InputEvent::PressEnter { .. } => this.search(Duration::ZERO, cx),
            InputEvent::Change => this.search(Duration::from_millis(350), cx),
            _ => {}
        })
        .detach();
        let missing = brew.is_none();
        let mut view = Self {
            brew,
            query,
            installed: Vec::new(),
            search_results: Vec::new(),
            search_task: None,
            search_id: 0,
            searching: false,
            search_error: None,
            busy: false,
            status: if missing {
                "Homebrew wasn’t found"
            } else {
                "Ready"
            }
            .into(),
            output: if missing {
                "Install Homebrew from https://brew.sh, then reopen this app.\nLooked in PATH, /opt/homebrew/bin and /usr/local/bin.".into()
            } else {
                "Homebrew output will appear here.".into()
            },
            failed: missing,
        };
        if !missing {
            view.refresh(false, cx);
        }
        view
    }

    fn summary(&self) -> String {
        let updates = self
            .installed
            .iter()
            .filter(|p| p.next_version.is_some())
            .count();
        format!(
            "{} installed · {updates} updates available",
            self.installed.len()
        )
    }

    fn refresh(&mut self, fetch_catalog: bool, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(brew) = self.brew.clone() else {
            return;
        };
        self.cancel_search();
        self.busy = true;
        self.failed = false;
        self.status = if fetch_catalog {
            "Checking for updates…"
        } else {
            "Reading installed packages…"
        }
        .into();
        let task = cx.background_spawn(async move {
            let update_error = if fetch_catalog {
                brew::update_catalog(&brew).err()
            } else {
                None
            };
            (brew::installed(&brew), update_error)
        });
        cx.spawn(async move |this, cx| {
            let (result, update_error) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(packages) => {
                        this.set_installed(packages);
                        this.status = this.summary();
                        this.output = "Package versions checked. Use Refresh to fetch the latest Homebrew metadata.".into();
                        if let Some(error) = update_error {
                            this.status = "Couldn’t check for updates — showing cached versions".into();
                            this.output = error;
                            this.failed = true;
                        }
                    }
                    Err(error) => { this.status = "Couldn’t load packages".into(); this.output = error; this.failed = true; }
                }
                if !this.query.read(cx).value().trim().is_empty() { this.search(Duration::ZERO, cx); }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn set_installed(&mut self, packages: Vec<Package>) {
        self.installed = packages;
        for package in &mut self.search_results {
            if let Some(installed) = self.installed.iter().find(|p| p.same_package(package)) {
                *package = installed.clone();
            } else {
                package.installed = false;
                package.version.clear();
                package.next_version = None;
                package.pinned = false;
            }
        }
    }

    fn cancel_search(&mut self) {
        self.search_task = None;
        self.search_id += 1;
        self.searching = false;
    }

    fn search(&mut self, delay: Duration, cx: &mut Context<Self>) {
        self.cancel_search();
        self.search_results.clear();
        self.search_error = None;
        let query = self.query.read(cx).value().trim().to_string();
        if self.busy || query.is_empty() {
            cx.notify();
            return;
        }
        let Some(brew) = self.brew.clone() else {
            cx.notify();
            return;
        };
        let installed = self.installed.clone();
        let search_id = self.search_id;
        self.searching = true;
        self.search_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let result = cx
                .background_spawn(async move { brew::search(&brew, &query, &installed) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.search_id != search_id {
                    return;
                }
                this.searching = false;
                match result {
                    Ok(packages) => this.search_results = packages,
                    Err(error) => this.search_error = Some(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn operate(&mut self, operation: Operation, package: Package, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(brew) = self.brew.clone() else {
            return;
        };
        self.cancel_search();
        self.busy = true;
        self.failed = false;
        self.status = format!("Running brew {} {}…", operation.command(), package.name);
        self.output = format!(
            "$ brew {} {} -- {}\n\nWorking… This can take a few minutes. Output appears when the command finishes.",
            operation.command(),
            package.kind.flag(),
            package.name
        );
        let task = cx.background_spawn(async move {
            let result = brew::execute(&brew, operation, &package);
            (result, brew::installed(&brew))
        });
        cx.spawn(async move |this, cx| {
            let (result, installed) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(output) => {
                        this.status = "Finished successfully".into();
                        this.output = output;
                    }
                    Err(error) => {
                        this.status = "Homebrew command failed".into();
                        this.output = error;
                        this.failed = true;
                    }
                }
                match installed {
                    Ok(packages) => this.set_installed(packages),
                    Err(error) => {
                        this.output
                            .push_str(&format!("\n\nCouldn’t refresh installed packages: {error}"));
                        this.status = "Package list may be out of date — refresh to retry".into();
                        this.failed = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn confirm_removal(&mut self, package: Package, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let package = package.clone();
            let view = view.clone();
            dialog.confirm().title(format!("Uninstall {}?", package.name))
                .description("Homebrew will remove this package. Packages that depend on it may prevent removal. App preferences are not explicitly deleted.")
                .ok_text("Uninstall")
                .on_ok(move |_, _, cx| {
                    view.update(cx, |this, cx| this.operate(Operation::Uninstall, package.clone(), cx));
                    true
                })
        });
    }

    fn package_row(
        &self,
        section: &'static str,
        index: usize,
        package: &Package,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut row = div()
            .id((section, index))
            .test_support()
            .role(Role::Group)
            .aria_label(package.name.clone())
            .p_4()
            .border_b_1()
            .border_color(cx.theme().border)
            .flex()
            .items_center()
            .gap_4()
            .child(
                div()
                    .size_9()
                    .rounded_lg()
                    .flex_shrink_0()
                    .bg(cx.theme().muted)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(if package.kind == Kind::Cask {
                            IconName::AppWindow
                        } else {
                            IconName::Terminal
                        })
                        .size_4()
                        .text_color(cx.theme().muted_foreground),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_ellipsis()
                            .child(package.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .text_ellipsis()
                            .child(if package.description.is_empty() {
                                package.kind.label().into()
                            } else {
                                package.description.clone()
                            }),
                    ),
            );
        if package.installed {
            let mut versions = div()
                .id(("versions", index))
                .test_support()
                .role(Role::Status)
                .aria_label(
                    package
                        .next_version
                        .as_ref()
                        .map(|next| format!("{} → {next}", package.version))
                        .unwrap_or_else(|| package.version.clone()),
                )
                .flex()
                .items_center()
                .gap_2()
                .child(Tag::secondary().small().child(package.version.clone()));
            if let Some(next) = &package.next_version {
                versions = versions
                    .child(
                        Icon::new(IconName::ArrowRight)
                            .size_3()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(Tag::success().outline().small().child(next.clone()));
            }
            if package.pinned {
                versions = versions.child(Tag::secondary().small().child("Pinned"));
            }
            row = row.child(versions);
            if package.next_version.is_some() {
                let package = package.clone();
                row = row.child(
                    Button::new(("update-package", index))
                        .primary()
                        .small()
                        .icon(IconName::ArrowUp)
                        .label("Update")
                        .disabled(self.busy || package.pinned)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.operate(Operation::Upgrade, package.clone(), cx)
                        })),
                );
            }
            let package = package.clone();
            row = row.child(
                Button::new(("uninstall-package", index))
                    .ghost()
                    .small()
                    .icon(IconName::Trash)
                    .label("Uninstall…")
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.confirm_removal(package.clone(), window, cx)
                    })),
            );
        } else {
            let package = package.clone();
            row = row.child(
                Button::new(("install-package", index))
                    .primary()
                    .small()
                    .icon(IconName::Download)
                    .label("Install")
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.operate(Operation::Install, package.clone(), cx)
                    })),
            );
        }
        row.into_any_element()
    }

    fn section_heading(
        &self,
        id: &'static str,
        title: &str,
        count: usize,
        cx: &App,
    ) -> impl IntoElement {
        div()
            .id(id)
            .test_support()
            .role(Role::Heading)
            .aria_label(title.to_string())
            .px_4()
            .pt_5()
            .pb_3()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.to_string()),
            )
            .child(Tag::secondary().small().child(count.to_string()))
            .text_color(cx.theme().muted_foreground)
    }
}

impl Render for BrewView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.query.read(cx).value().trim().to_lowercase();
        let installed: Vec<_> = self
            .installed
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                filter.is_empty()
                    || p.name.to_lowercase().contains(&filter)
                    || p.description.to_lowercase().contains(&filter)
            })
            .collect();
        let available: Vec<_> = self
            .search_results
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.installed)
            .collect();
        let mut list =
            div()
                .id("package-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_6()
                .pb_6()
                .child(self.section_heading("installed-section", "Installed", installed.len(), cx))
                .children(installed.iter().map(|(index, package)| {
                    self.package_row("installed-package", *index, package, cx)
                }))
                .when(installed.is_empty(), |list| {
                    list.child(
                        div()
                            .p_4()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(if self.busy {
                                "Reading installed packages…"
                            } else if self.brew.is_none() {
                                "Install Homebrew to get started."
                            } else if filter.is_empty() {
                                "No installed packages."
                            } else {
                                "No installed packages match your search."
                            }),
                    )
                });
        if !filter.is_empty() {
            list = list
                .child(self.section_heading(
                    "available-section",
                    "Available packages",
                    available.len(),
                    cx,
                ))
                .when(self.searching, |list| {
                    list.child(
                        div()
                            .p_4()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Searching Homebrew…"),
                    )
                })
                .when_some(self.search_error.clone(), |list, error| {
                    list.child(
                        div()
                            .p_4()
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(error),
                    )
                })
                .children(available.into_iter().map(|(index, package)| {
                    self.package_row("available-package", index, package, cx)
                }))
                .when(
                    !self.searching
                        && self.search_error.is_none()
                        && self.search_results.iter().all(|p| p.installed),
                    |list| {
                        list.child(
                            div()
                                .p_4()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No additional packages found."),
                        )
                    },
                );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .px_8()
                    .pt_6()
                    .pb_5()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .size_9()
                                    .rounded_lg()
                                    .bg(rgb(0xe9b461))
                                    .text_color(rgb(0x3d2b12))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(Icon::new(IconName::Beer).size_5()),
                            )
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Homebrew"),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("refresh-packages")
                                    .outline()
                                    .small()
                                    .icon(IconName::RefreshCw)
                                    .label("Refresh")
                                    .disabled(self.busy || self.brew.is_none())
                                    .on_click(cx.listener(|this, _, _, cx| this.refresh(true, cx))),
                            ),
                    )
                    .child(
                        Input::new(&self.query)
                            .id("package-search")
                            .prefix(IconName::Search)
                            .disabled(self.busy),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(list),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .px_8()
                    .py_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Icon::new(IconName::Activity).size_4().text_color(
                                if self.failed {
                                    cx.theme().danger
                                } else {
                                    cx.theme().muted_foreground
                                },
                            ))
                            .child(
                                div()
                                    .id("brew-status")
                                    .test_support()
                                    .role(Role::Status)
                                    .aria_label(self.status.clone())
                                    .text_sm()
                                    .child(self.status.clone()),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if self.busy {
                                        "Please keep this window open"
                                    } else {
                                        "ACTIVITY"
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("brew-output")
                            .h(px(65.))
                            .overflow_y_scroll()
                            .rounded_md()
                            .bg(cx.theme().muted)
                            .p_3()
                            .text_xs()
                            .font_family("Menlo")
                            .text_color(if self.failed {
                                cx.theme().danger
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(self.output.clone()),
                    ),
            )
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::BrewView;
    use gpui_kit::{
        AppContext, TestAppContext,
        component::Root,
        px, size,
        test::{TestAppContextExt, TestWindowExt},
    };
    use std::{os::unix::fs::PermissionsExt, time::Duration};

    #[gpui_kit::test]
    async fn combined_search_updates_and_refresh_preserve_installed_packages(
        cx: &mut TestAppContext,
    ) {
        let directory =
            std::env::temp_dir().join(format!("homebrew-gui-view-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let brew = directory.join("brew");
        std::fs::write(&brew, r##"#!/bin/sh
case "$1" in
  info)
    if [ -f "$0.upgraded" ]; then version=1.1; outdated=false; else version=1.0; outdated=true; fi
    printf '{"formulae":[{"full_name":"wget","desc":"Download files","installed":[{"version":"%s"}],"versions":{"stable":"1.1"},"outdated":%s}' "$version" "$outdated"
    if [ -f "$0.installed" ]; then
      printf ',{"full_name":"wget2","desc":"Another downloader","installed":[{"version":"2.0"}],"versions":{"stable":"2.0"},"outdated":false}'
    fi
    printf '],"casks":[{"token":"wget-app","desc":"Downloader app","installed":"1.0","version":"2.0","outdated":true,"pinned":true}]}';;
  search)
    if [ "$2" = --formula ]; then printf '%s\n' wget wget2;
    else printf '%s\n' wget-app wget-studio; fi;;
  update)
    if [ -f "$0.offline" ]; then echo 'Network unavailable' >&2; exit 9; fi
    touch "$0.checked";;
  install) touch "$0.installed"; echo 'Installed wget2';;
  uninstall) touch "$0.removed";;
  upgrade)
    if [ "$4" != wget ]; then echo 'Unexpected update target' >&2; exit 8; fi
    if [ "$HOMEBREW_NO_ASK" != 1 ]; then echo 'Missing noninteractive confirmation' >&2; exit 8; fi
    touch "$0.upgraded"; echo 'Updated wget';;
esac
"##).unwrap();
        std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o700)).unwrap();
        cx.update(gpui_kit::init);
        let mut view = None;
        let handle = cx.open_window(size(px(1000.), px(720.)), |window, cx| {
            let entity = cx.new(|cx| BrewView::new(Some(brew.clone()), window, cx));
            view = Some(entity.clone());
            Root::new(entity, window, cx)
        });
        let view = view.unwrap();
        cx.wait_for(handle.into(), Duration::from_secs(5), |_, cx| {
            !view.read(cx).busy
        })
        .await;
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(view.read(cx).installed.len(), 2);
            assert!(window.try_find("installed-section").is_some());
            assert!(window.try_find("available-section").is_none());
            assert!(window.try_find("installed-tab").is_none());
            assert_eq!(window.find(("versions", 0usize)).label(), Some("1.0 → 1.1"));
            window.click(("update-package", 1usize), cx);
            assert!(!view.read(cx).busy, "Pinned packages must not be updated");
            window.click(("update-package", 0usize), cx);
            assert!(view.read(cx).busy);
        })
        .unwrap();
        cx.wait_for(handle.into(), Duration::from_secs(5), |_, cx| {
            !view.read(cx).busy
        })
        .await;
        assert!(brew.with_extension("upgraded").exists());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(view.read(cx).installed[0].version, "1.1");
            assert!(window.try_find(("update-package", 0usize)).is_none());
            window.click(("uninstall-package", 0usize), cx);
            window.render_frame(cx);
            window.click("cancel", cx);
            assert!(!brew.with_extension("removed").exists());
            window.click("package-search", cx);
            window.input("wget", cx);
            window.render_frame(cx);
            assert!(window.try_find("installed-section").is_some());
            assert!(window.try_find("available-section").is_some());
        })
        .unwrap();
        // The debounce uses the test executor's clock.
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.wait_for(handle.into(), Duration::from_secs(5), |_, cx| {
            !view.read(cx).searching
        })
        .await;
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(view.read(cx).search_results.len(), 4);
            assert!(window.try_find(("installed-package", 0usize)).is_some());
            assert!(window.try_find(("installed-package", 1usize)).is_some());
            assert!(
                window.try_find(("available-package", 0usize)).is_none(),
                "Installed results must not be duplicated"
            );
            assert!(window.try_find(("available-package", 1usize)).is_none());
            assert!(window.try_find(("available-package", 2usize)).is_some());
            window.click(("install-package", 3usize), cx);
            assert!(view.read(cx).busy);
        })
        .unwrap();
        cx.wait_for(handle.into(), Duration::from_secs(5), |_, cx| {
            !view.read(cx).busy
        })
        .await;
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(view.read(cx).installed.len(), 3);
            assert!(window.try_find(("available-package", 3usize)).is_none());
            assert!(window.try_find(("installed-package", 2usize)).is_some());
            window.click("package-search", cx);
            window.press("secondary-a", cx);
            window.input("no-match", cx);
            window.render_frame(cx);
            assert!(window.try_find(("installed-package", 0usize)).is_none());
            window.press("secondary-a", cx);
            window.press("backspace", cx);
            window.render_frame(cx);
            assert!(window.try_find("available-section").is_none());
            assert!(window.try_find(("installed-package", 2usize)).is_some());
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(!view.read(cx).searching);
            assert!(view.read(cx).search_results.is_empty());
            window.click("refresh-packages", cx);
        })
        .unwrap();
        cx.wait_for(handle.into(), Duration::from_secs(5), |_, cx| {
            !view.read(cx).busy
        })
        .await;
        assert!(brew.with_extension("checked").exists());
        std::fs::write(brew.with_extension("offline"), "").unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("refresh-packages", cx);
        })
        .unwrap();
        cx.wait_for(handle.into(), Duration::from_secs(5), |_, cx| {
            !view.read(cx).busy
        })
        .await;
        cx.update(|cx| {
            assert!(view.read(cx).failed);
            assert!(view.read(cx).status.contains("cached versions"));
            assert!(view.read(cx).output.contains("Network unavailable"));
            assert_eq!(view.read(cx).installed.len(), 3);
        });
        std::fs::remove_dir_all(directory).unwrap();
    }
}
