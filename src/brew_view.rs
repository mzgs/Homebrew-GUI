use crate::brew::{self, Kind, Operation, Package};
use gpui_kit::{
    assets::IconName,
    base::{ScrollBounce, ScrollBounceMotion},
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

#[derive(PartialEq, Eq)]
enum PackageListRow {
    Heading(&'static str, &'static str, usize),
    Installed(usize),
    Available(usize),
    Message(String, bool),
}

pub(crate) struct BrewView {
    brew: Option<PathBuf>,
    query: Entity<InputState>,
    list_scroll: ListState,
    list_rows: Vec<PackageListRow>,
    installed: Vec<Package>,
    search_results: Vec<Package>,
    search_task: Option<Task<()>>,
    search_id: u64,
    searching: bool,
    search_error: Option<String>,
    pub(crate) busy: bool,
    status: String,
    output: String,
    output_scroll: ScrollHandle,
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
            // Measure beyond the viewport so wheel scrolling has content to advance into.
            list_scroll: ListState::new(0, ListAlignment::Top, px(500.)),
            list_rows: Vec::new(),
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
            output_scroll: ScrollHandle::new(),
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
        let (sender, output_task) = self.stream_output(
            if fetch_catalog {
                "$ brew update\n\n"
            } else {
                "Reading installed packages…\n"
            }
            .into(),
            cx,
        );
        let task = cx.background_spawn(async move {
            let update_error = if fetch_catalog {
                brew::update_catalog(&brew, |output| {
                    let _ = sender.force_send(output.into());
                })
                .err()
            } else {
                None
            };
            drop(sender);
            (brew::installed(&brew), update_error)
        });
        cx.spawn(async move |this, cx| {
            output_task.await;
            let (result, update_error) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(packages) => {
                        this.set_installed(packages);
                        this.status = this.summary();
                        this.output.push_str("\nPackage versions checked. Use Refresh to fetch the latest Homebrew metadata.");
                        if let Some(error) = update_error {
                            this.status = "Couldn’t check for updates — showing cached versions".into();
                            this.output = error;
                            this.failed = true;
                        }
                    }
                    Err(error) => { this.status = "Couldn’t load packages".into(); this.output = error; this.failed = true; }
                }
                this.output_scroll.scroll_to_bottom();
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
        let (sender, output_task) = self.stream_output(
            format!(
                "$ brew {} {} -- {}\n\n",
                operation.command(),
                package.kind.flag(),
                package.name
            ),
            cx,
        );
        let task = cx.background_spawn(async move {
            let result = brew::execute(&brew, operation, &package, |output| {
                let _ = sender.force_send(output.into());
            });
            drop(sender);
            (result, brew::installed(&brew))
        });
        cx.spawn(async move |this, cx| {
            output_task.await;
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
                this.output_scroll.scroll_to_bottom();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn stream_output(
        &mut self,
        header: String,
        cx: &mut Context<Self>,
    ) -> (async_channel::Sender<String>, Task<()>) {
        self.output = header.clone();
        self.output_scroll.scroll_to_bottom();
        let (sender, receiver) = async_channel::bounded::<String>(1);
        let task = cx.spawn(async move |this, cx| {
            while let Ok(output) = receiver.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.output = format!("{header}{output}");
                        this.output_scroll.scroll_to_bottom();
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        (sender, task)
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
                .aria_label(package.version.clone())
                .flex()
                .items_center()
                .gap_2()
                .child(Tag::secondary().small().child(package.version.clone()));
            if package.pinned {
                versions = versions.child(Tag::secondary().small().child("Pinned"));
            }
            if package.next_version.is_some() {
                let package = package.clone();
                row = row.child(
                    Button::new(("update-package", index))
                        .primary()
                        .small()
                        .icon(IconName::ArrowUp)
                        .label("Update")
                        .disabled(self.busy || package.pinned)
                        .when(!self.busy && !package.pinned, |button| {
                            button.cursor_pointer()
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.operate(Operation::Upgrade, package.clone(), cx)
                        })),
                );
            }
            row = row.child(versions);
            let package = package.clone();
            row = row.child(
                Button::new(("uninstall-package", index))
                    .ghost()
                    .small()
                    .icon(IconName::Trash)
                    .accessibility_label("Uninstall…")
                    .disabled(self.busy)
                    .when(!self.busy, |button| button.cursor_pointer())
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
                    .when(!self.busy, |button| button.cursor_pointer())
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
        let mut rows = vec![PackageListRow::Heading(
            "installed-section",
            "Installed",
            installed.len(),
        )];
        rows.extend(
            installed
                .iter()
                .map(|(index, _)| PackageListRow::Installed(*index)),
        );
        if installed.is_empty() {
            rows.push(PackageListRow::Message(
                if self.busy {
                    "Reading installed packages…"
                } else if self.brew.is_none() {
                    "Install Homebrew to get started."
                } else if filter.is_empty() {
                    "No installed packages."
                } else {
                    "No installed packages match your search."
                }
                .into(),
                false,
            ));
        }
        if !filter.is_empty() {
            rows.push(PackageListRow::Heading(
                "available-section",
                "Available packages",
                available.len(),
            ));
            if self.searching {
                rows.push(PackageListRow::Message("Searching Homebrew…".into(), false));
            }
            if let Some(error) = &self.search_error {
                rows.push(PackageListRow::Message(error.clone(), true));
            }
            rows.extend(
                available
                    .iter()
                    .map(|(index, _)| PackageListRow::Available(*index)),
            );
            if !self.searching && self.search_error.is_none() && available.is_empty() {
                rows.push(PackageListRow::Message(
                    "No additional packages found.".into(),
                    false,
                ));
            }
        }
        if rows != self.list_rows {
            self.list_scroll.reset(rows.len());
            self.list_rows = rows;
        }
        let list = list(
            self.list_scroll.clone(),
            cx.processor(|this, index, _, cx| {
                let row = match &this.list_rows[index] {
                    PackageListRow::Heading(id, title, count) => this
                        .section_heading(id, title, *count, cx)
                        .into_any_element(),
                    PackageListRow::Installed(index) => {
                        this.package_row("installed-package", *index, &this.installed[*index], cx)
                    }
                    PackageListRow::Available(index) => this.package_row(
                        "available-package",
                        *index,
                        &this.search_results[*index],
                        cx,
                    ),
                    PackageListRow::Message(message, failed) => div()
                        .p_4()
                        .text_sm()
                        .text_color(if *failed {
                            cx.theme().danger
                        } else {
                            cx.theme().muted_foreground
                        })
                        .child(message.clone())
                        .into_any_element(),
                };
                div().px_6().child(row).into_any_element()
            }),
        )
        .flex_1()
        .min_h_0()
        .pb_6();
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
                                    .when(!self.busy && self.brew.is_some(), |button| {
                                        button.cursor_pointer()
                                    })
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
                    .child(
                        ScrollBounce::new("package-list-bounce", &self.list_scroll, list)
                            .enabled(true)
                            .motion(ScrollBounceMotion::default().with_tracking(0.25)),
                    ),
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
                            .test_support()
                            .h(px(140.))
                            .overflow_y_scroll()
                            .track_scroll(&self.output_scroll)
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
        AppContext, InputEvent, TestAppContext,
        component::Root,
        px, size,
        test::{TestAppContextExt, TestWindowExt},
    };
    use std::{os::unix::fs::PermissionsExt, time::Duration};

    #[gpui_kit::test]
    fn activity_stream_updates_while_busy(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let mut view = None;
        let handle = cx.open_window(size(px(1000.), px(720.)), |window, cx| {
            let entity = cx.new(|cx| BrewView::new(None, window, cx));
            view = Some(entity.clone());
            Root::new(entity, window, cx)
        });
        let view = view.unwrap();
        let (sender, _task) = view.update(cx, |view, cx| {
            view.busy = true;
            view.stream_output("$ brew install wget\n\n".into(), cx)
        });
        sender.force_send("Downloading…\n".repeat(40)).unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let view = view.read(cx);
            assert!(view.busy);
            assert!(view.output.contains("Downloading…"));
            assert!(view.output_scroll.max_offset().y > px(0.));
            assert_eq!(
                view.output_scroll.offset().y,
                -view.output_scroll.max_offset().y
            );
        })
        .unwrap();
        sender
            .force_send("Downloading…\nInstalling café…".into())
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(view.read(cx).busy);
            assert!(view.read(cx).output.ends_with("Installing café…"));
        });
        drop(sender);
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn package_list_virtualizes_and_filters_large_inventories(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let mut view = None;
        let handle = cx.open_window(size(px(1000.), px(720.)), |window, cx| {
            let entity = cx.new(|cx| {
                let mut view = BrewView::new(None, window, cx);
                view.installed = (0..1000)
                    .map(|index| crate::brew::Package {
                        name: format!("package-{index:04}"),
                        kind: crate::brew::Kind::Formula,
                        description: "Test package".into(),
                        version: "1.0".into(),
                        installed: true,
                        next_version: None,
                        pinned: false,
                    })
                    .collect();
                view
            });
            view = Some(entity.clone());
            Root::new(entity, window, cx)
        });
        let view = view.unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find(("installed-package", 0usize)).is_some());
            assert!(window.try_find(("installed-package", 999usize)).is_none());
            window.scroll(
                ("installed-package", 0usize),
                gpui_kit::ScrollDelta::Lines(gpui_kit::point(0., -10.)),
                cx,
            );
            assert!(
                view.read(cx).list_scroll.logical_scroll_top().item_ix > 0,
                "Wheel input must advance the virtual list"
            );
            for _ in 0..12 {
                let index = view.read(cx).list_scroll.logical_scroll_top().item_ix;
                window.scroll(
                    ("installed-package", index),
                    gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-200.))),
                    cx,
                );
            }
            let scrolled = view.read(cx).list_scroll.logical_scroll_top().item_ix;
            assert!(
                scrolled > 20,
                "Trackpad scrolling must keep advancing beyond the buffer"
            );
            window.scroll(
                ("installed-package", scrolled),
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(200.))),
                cx,
            );
            assert!(
                view.read(cx).list_scroll.logical_scroll_top().item_ix < scrolled,
                "Trackpad scrolling must also work upward"
            );
            view.read(cx).list_scroll.scroll_to(gpui_kit::ListOffset {
                item_ix: 1000,
                offset_in_item: px(0.),
            });
            window.refresh();
            window.render_frame(cx);
            assert!(window.try_find(("installed-package", 0usize)).is_none());
            assert!(window.try_find(("installed-package", 999usize)).is_some());
            window.click("package-search", cx);
            window.input("package-0999", cx);
            window.render_frame(cx);
            assert!(window.try_find("installed-section").is_some());
            assert!(window.try_find(("installed-package", 999usize)).is_some());
            assert!(window.try_find(("installed-package", 998usize)).is_none());
        })
        .unwrap();
    }

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
    touch "$0.upgraded"
    line=0
    while [ "$line" -lt 40 ]; do echo 'Updating wget'; line=$((line + 1)); done
    echo 'Updated wget';;
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
        cx.update(|cx| {
            use gpui_kit::component::{Theme, ThemeMode};
            for mode in [ThemeMode::Dark, ThemeMode::Light] {
                Theme::change(mode, None, cx);
                crate::configure_button_feedback(cx);
                let theme = Theme::global(cx);
                assert!(theme.button_primary_hover.l > theme.primary.l);
                assert_eq!(theme.button_primary_hover.h, theme.primary.h);
                assert_eq!(theme.button_primary_hover.s, theme.primary.s);
            }
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(view.read(cx).installed.len(), 2);
            assert!(window.try_find("installed-section").is_some());
            assert!(window.try_find("available-section").is_none());
            assert!(window.try_find("installed-tab").is_none());
            assert_eq!(window.find(("versions", 0usize)).label(), Some("1.0"));
            assert_eq!(
                window.find(("update-package", 0usize)).label(),
                Some("Update")
            );
            let button_bounds = window
                .find(("update-package", 0usize))
                .bounds()
                .scale(window.scale_factor());
            let button_background = |window: &gpui_kit::Window| {
                window
                    .painted_quads()
                    .into_iter()
                    .find(|quad| quad.bounds == button_bounds)
                    .expect("Update button must paint a background")
                    .background
            };
            let normal = button_background(window);
            window.dispatch_event(
                gpui_kit::MouseMoveEvent {
                    position: window.find(("update-package", 0usize)).bounds().center(),
                    pressed_button: None,
                    modifiers: Default::default(),
                }
                .to_platform_input(),
                cx,
            );
            window.draw(cx).clear(cx);
            assert_ne!(
                button_background(window),
                normal,
                "Hover must change the Update button background"
            );
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
            let scroll = view.read(cx).output_scroll.clone();
            assert!(scroll.max_offset().y > px(0.));
            assert_eq!(scroll.offset().y, -scroll.max_offset().y);
            window.scroll(
                "brew-output",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(40.))),
                cx,
            );
            let offset = scroll.offset().y;
            assert!(
                offset > -scroll.max_offset().y,
                "Older output stays scrollable"
            );
            window.refresh();
            window.render_frame(cx);
            assert_eq!(
                scroll.offset().y,
                offset,
                "Redraws preserve manual scrolling"
            );
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
