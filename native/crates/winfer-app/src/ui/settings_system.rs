//! Settings → Backup & transfer and Diagnostics, in tabs (mockups/settings-overhaul/data.html and
//! diagnostics.html): Export / Import, and Overview / Logs / Crashes.

use super::settings_kit::{PageTab, alpha, badge, banner, empty_state, icon_tile, intro, meter, panel, section, small_label, tab_count, tab_dot};
use super::*;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::tooltip::Tooltip;
use serde_json::{Value, json};
use winfer_backend::{diagnostics, transfer};

/// How a [`button`] looks.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Look {
    Plain,
    Primary,
    /// Outlined in the danger color (clears or deletes something).
    Danger,
    /// Filled with the danger color (the step that replaces data).
    DangerFill,
}

/// A 28px button with an optional icon; dimmed and inert-looking when not `enabled`.
pub(super) fn button(c: Colors, id: impl Into<ElementId>, icon: Option<IconName>, label: impl Into<SharedString>, look: Look, enabled: bool) -> Stateful<Div> {
    let d = div().id(id).h(px(28.0)).flex().flex_shrink_0().items_center().gap_1p5().rounded_md().px_2p5().text_size(px(12.0));
    let d = match look {
        Look::Plain => d.border_1().border_color(c.edge).bg(c.raised).text_color(c.fg).when(enabled, |d| d.hover(move |d| d.bg(c.hover))),
        Look::Primary => d.bg(c.accent).text_color(c.on_accent).font_weight(FontWeight::MEDIUM).when(enabled, |d| d.hover(|d| d.opacity(0.9))),
        Look::Danger => d.border_1().border_color(alpha(c.danger, 0.55)).text_color(c.danger).when(enabled, |d| d.hover(move |d| d.bg(alpha(c.danger, 0.1)))),
        Look::DangerFill => d.bg(c.danger).text_color(gpui_kit::white()).font_weight(FontWeight::MEDIUM).when(enabled, |d| d.hover(|d| d.opacity(0.9))),
    };
    d.when(enabled, |d| d.cursor_pointer()).when(!enabled, |d| d.opacity(0.45)).children(icon.map(|i| Icon::new(i).size(px(13.0)))).child(label.into())
}

/// A success or error line with an icon and optional actions, inside the flow of a tab.
pub(super) fn result_banner(c: Colors, result: &Result<String, String>, actions: Option<AnyElement>) -> Div {
    let (text, color, icon) = match result {
        Ok(t) => (t.clone(), c.success, IconName::CircleCheck),
        Err(t) => (t.clone(), c.danger, IconName::CircleX),
    };
    banner(color, icon, text).children(actions)
}

/// "412 MB", "38 KB", "4.2 MB".
fn bytes_label(bytes: f64) -> String {
    let bytes = if bytes <= 0.0 { 0.0 } else { bytes };
    if bytes < 1048576.0 {
        format!("{:.0} KB", (bytes / 1024.0).ceil())
    } else if bytes < 10.0 * 1048576.0 {
        format!("{:.1} MB", bytes / 1048576.0)
    } else {
        format!("{:.0} MB", bytes / 1048576.0)
    }
}

/// `YYYY-MM-DD` of a Unix time in seconds (UTC).
fn date_of(secs: i64) -> String {
    let days = secs.div_euclid(86400) + 719468;
    let era = days.div_euclid(146097);
    let doe = days.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02}")
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64
}

/// Today as `YYYY-MM-DD`, for file names.
pub(super) fn chrono_date() -> String {
    date_of(now_secs())
}

/// The MB numbers of the memory samples ("2026-11-22T10:15:03Z 412MB").
fn sample_values(lines: &[String]) -> Vec<f32> {
    lines.iter().filter_map(|line| line.trim_end().strip_suffix("MB")?.rsplit(' ').next()?.parse().ok()).collect()
}

/// "2 h 14 min", "38 min".
fn uptime(secs: f64) -> String {
    let min = (secs / 60.0).floor() as u64;
    if min >= 60 { format!("{} h {} min", min / 60, min % 60) } else { format!("{min} min") }
}

fn os_name(os: &str) -> String {
    match os {
        "macos" => "macOS".into(),
        "windows" => "Windows".into(),
        "linux" => "Linux".into(),
        other => other.to_string(),
    }
}

/// The minidumps in the log folder, newest first: name, bytes, modified (Unix seconds).
fn minidumps(dir: &str) -> Vec<Value> {
    let mut dumps: Vec<(String, u64, i64)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let meta = e.metadata().ok()?;
            let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
            name.ends_with(".dmp").then_some((name, meta.len(), modified))
        })
        .collect();
    dumps.sort_by_key(|d| std::cmp::Reverse(d.2));
    dumps.into_iter().map(|(name, bytes, modified)| json!({ "name": name, "bytes": bytes, "modified": modified })).collect()
}

impl Root {
    // ───────── Backup & transfer ─────────

    pub(super) fn data_tabs(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        self.page_tab_bar("data", "export", vec![PageTab::new("export", "Export", IconName::Upload), PageTab::new("import", "Import", IconName::Download)], c, cx)
    }

    pub(super) fn page_data(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let body = if self.page_tab("data", "export") == "import" { self.data_import(c, cx) } else { self.data_export(c, cx) };
        body.pt_3()
    }

    fn data_export(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let ui = self.settings.as_ref().unwrap();
        let (with_secrets, busy) = (ui.with_secrets, ui.busy);
        let (pass, conf) = (ui.passphrase.read(cx).value().to_string(), ui.confirm.read(cx).value().to_string());
        let (long_enough, matching) = (pass.len() >= 8, !conf.is_empty() && pass == conf);
        let can_export = !busy && (!with_secrets || (long_enough && matching));
        let (passphrase, confirm) = (ui.passphrase.clone(), ui.confirm.clone());
        let result = ui.export_result.clone();
        let mono = crate::fonts::mono(cx);
        let workspaces = self.state.read(cx).workspaces.len();

        let included = |icon: IconName, title: &str, sub: String| {
            panel(c).flex_1().min_w_0().gap_1p5().px_3().py_2p5()
                .child(div().flex().items_center().gap_2().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg)
                    .child(icon_tile(c.muted, icon, 24.0)).child(div().flex_1().min_w_0().truncate().child(title.to_string())).child(Icon::new(IconName::Check).size(px(13.0)).text_color(c.success)))
                .child(div().text_size(px(11.5)).text_color(c.subtle).child(sub))
        };
        let tiles = div().flex().gap_2p5()
            .child(included(IconName::SlidersHorizontal, "Settings", "Every page here, shortcuts and themes".into()))
            .child(included(IconName::Folder, "Workspaces", if workspaces == 1 { "1 workspace and its panes".into() } else { format!("{workspaces} workspaces and their panes") }))
            .child(included(IconName::Puzzle, "Plugins", "Plugins with their data".into()))
            .child(included(IconName::ChartColumn, "Usage history", "The last 90 days".into()));

        let check = |ok: bool, text: &str| div().flex().items_center().gap_1p5().text_size(px(11.5)).text_color(if ok { c.success } else { c.subtle })
            .child(Icon::new(if ok { IconName::CircleCheck } else { IconName::Circle }).size(px(12.0))).child(text.to_string());
        let field = |label: &str, input: &Entity<InputState>| div().flex_1().min_w_0().flex().flex_col().gap_1()
            .child(div().text_size(px(11.5)).text_color(c.subtle).child(label.to_string()))
            .child(Input::new(input));
        let secrets_toggle = self.toggle("export-secrets", with_secrets, c, cx, |this, _| {
            if let Some(s) = this.settings.as_mut() {
                s.with_secrets = !s.with_secrets;
            }
        });
        let secrets = panel(c)
            .child(div().flex().items_center().gap_3().px_3p5().py_3()
                .child(icon_tile(c.warning, IconName::KeyRound, 28.0))
                .child(small_label(c, "Include secrets", "ChatGPT sign-in, the voice OpenAI key and plugin vault values, encrypted with a passphrase"))
                .child(secrets_toggle))
            .when(with_secrets, |d| d.child(div().flex().flex_col().gap_2p5().px_3p5().pt_3().pb_3p5().border_t_1().border_color(c.border)
                .child(div().flex().gap_3().child(field("Passphrase", &passphrase)).child(field("Repeat", &confirm)))
                .child(div().flex().gap_4().child(check(long_enough, "8 characters or more")).child(check(matching, "Both match")))
                .child(div().flex().items_center().gap_1p5().text_size(px(11.5)).text_color(c.warning)
                    .child(Icon::new(IconName::TriangleAlert).size(px(12.0)))
                    .child("The passphrase can't be recovered. Without it, the file imports without its secrets."))));

        let file = format!("winfer-{}.winfer", chrono_date());
        let action = panel(c).child(div().flex().items_center().gap_3().px_3p5().py_3()
            .child(icon_tile(c.accent, IconName::FileArchive, 28.0))
            .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                .child(div().truncate().font_family(mono).text_size(px(12.5)).text_color(c.fg).child(file))
                .child(div().text_size(px(11.5)).text_color(c.subtle).child(if with_secrets { "Settings, workspaces, plugins and usage · secrets encrypted" } else { "Settings, workspaces, plugins and usage · no secrets" })))
            .child(button(c, "export", Some(IconName::Upload), if busy { "Working…" } else { "Export…" }, Look::Primary, can_export).on_click(cx.listener(move |this, _, _, cx| {
                if can_export {
                    this.run_export(cx);
                }
            }))));

        let result = result.map(|r| {
            let actions = match &r {
                Ok(path) => {
                    let path = path.clone();
                    button(c, "export-reveal", Some(IconName::FolderOpen), "Show in Finder", Look::Plain, true).on_click(cx.listener(move |_, _, _, _| sidebar::reveal_path(&path))).into_any_element()
                }
                Err(_) => button(c, "export-retry", Some(IconName::RotateCw), "Try again", Look::Plain, can_export).on_click(cx.listener(move |this, _, _, cx| {
                    if can_export {
                        this.run_export(cx);
                    }
                })).into_any_element(),
            };
            let shown = r.map(|p| format!("Exported to {p}")).map_err(|e| format!("Export failed: {e}"));
            result_banner(c, &shown, Some(actions))
        });
        div().flex().flex_col().gap_4()
            .child(intro(c, "One file you can import on another machine, or keep as a backup.", None))
            .child(section(c, "In the file", None))
            .child(tiles)
            .child(secrets)
            .child(action)
            .children(result)
    }

    fn data_import(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let ui = self.settings.as_ref().unwrap();
        let importing = ui.importing.clone();
        let import_pass = ui.import_passphrase.clone();
        let error = ui.import_result.clone().filter(Result::is_err);
        let mono = crate::fonts::mono(cx);
        let mut body = div().flex().flex_col().gap_4();
        let Some((path, manifest)) = importing else {
            let drop = div().id("import-drop").flex().flex_col().items_center().gap_2().py(px(34.0)).rounded(px(10.0)).border_1().border_dashed().border_color(c.border_strong).cursor_pointer()
                .hover(move |d| d.bg(c.hover))
                .drag_over::<ExternalPaths>(move |d, _, _, _| d.border_color(c.accent).bg(alpha(c.accent, 0.06)))
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                    if let Some(p) = paths.paths().first() {
                        this.inspect_import(p.display().to_string(), cx);
                    }
                }))
                .on_click(cx.listener(|this, _, _, cx| this.pick_import(cx)))
                .child(icon_tile(c.accent, IconName::FileDown, 34.0))
                .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child("Drop a .winfer file here"))
                .child(div().flex().gap_1().text_size(px(11.5)).text_color(c.subtle).child("or").child(div().text_color(c.accent).child("choose one…")).child("· made with Export on this or another machine"));
            let note = panel(c).flex_row().items_start().gap_2p5().px_3().py_2p5().text_size(px(12.0)).text_color(c.muted)
                .child(Icon::new(IconName::Lightbulb).size(px(15.0)).text_color(c.accent).flex_shrink_0())
                .child("An export is also a backup: make one before big changes and import it to go back. Importing never merges; it replaces.");
            return body.child(drop).children(error.map(|e| result_banner(c, &e, None))).child(note);
        };
        let name = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_string();
        let has_secrets = manifest["hasSecrets"] == true;
        let user = manifest["home"].as_str().and_then(|h| h.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next()).unwrap_or_default().to_string();
        let mut facts = Vec::new();
        if !user.is_empty() {
            facts.push(format!("From {user}'s {}", os_name(manifest["platform"].as_str().unwrap_or("computer"))));
        }
        if let Some(ms) = manifest["createdAt"].as_f64() {
            facts.push(date_of((ms / 1000.0) as i64));
        }
        if let Some(v) = manifest["appVersion"].as_str() {
            facts.push(format!("winfer {v}"));
        }
        if let Ok(meta) = std::fs::metadata(&path) {
            facts.push(bytes_label(meta.len() as f64));
        }
        let holds = |icon: IconName, text: &str, color: Hsla| div().flex().items_center().gap_1p5().px_2().h(px(24.0)).rounded_md().bg(c.app).text_size(px(11.5)).text_color(color).child(Icon::new(icon).size(px(12.0))).child(text.to_string());
        let mut card = panel(c)
            .child(div().flex().items_center().gap_3().px_3p5().py_3()
                .child(icon_tile(c.accent, IconName::FileArchive, 34.0))
                .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                    .child(div().truncate().font_family(mono).text_size(px(13.0)).text_color(c.fg).child(name))
                    .child(div().truncate().text_size(px(11.5)).text_color(c.subtle).child(facts.join(" · "))))
                .child(button(c, "import-choose", Some(IconName::FolderOpen), "Change…", Look::Plain, true).on_click(cx.listener(|this, _, _, cx| this.pick_import(cx)))))
            .child(div().flex().flex_wrap().gap_1p5().px_3p5().pb_3()
                .child(holds(IconName::SlidersHorizontal, "Settings", c.muted))
                .child(holds(IconName::Folder, "Workspaces", c.muted))
                .child(holds(IconName::Puzzle, "Plugins", c.muted))
                .child(holds(IconName::ChartColumn, "Usage history", c.muted))
                .child(if has_secrets { holds(IconName::KeyRound, "Encrypted secrets", c.warning) } else { holds(IconName::KeyRound, "No secrets", c.subtle) }));
        if has_secrets {
            card = card.child(div().flex().items_center().gap_3().px_3p5().py_3().border_t_1().border_color(c.border)
                .child(icon_tile(c.warning, IconName::Lock, 28.0))
                .child(small_label(c, "Passphrase for the secrets", "Leave empty to import everything except the secrets."))
                .child(div().w(px(200.0)).child(Input::new(&import_pass))));
        }
        let danger = div().flex().items_center().gap_3().px_3p5().py_3().rounded(px(10.0)).border_1().border_color(alpha(c.danger, 0.4)).bg(alpha(c.danger, 0.05))
            .child(icon_tile(c.danger, IconName::TriangleAlert, 28.0))
            .child(small_label(c, "Replace this machine's data", "Settings, workspaces, plugins and usage history here are replaced by the file's. winfer then restarts, which stops the agents in your panes."))
            .child(button(c, "import-run", Some(IconName::RefreshCcw), "Import and restart", Look::DangerFill, true).on_click(cx.listener(|this, _, window, cx| this.run_import(window, cx))));
        body = body.child(card).child(danger).children(error.map(|e| result_banner(c, &e, None)));
        body
    }

    pub(super) fn run_export(&mut self, cx: &mut Context<Self>) {
        let Some(ui) = self.settings.as_mut() else { return };
        let secret = ui.with_secrets.then(|| ui.passphrase.read(cx).value().to_string());
        let dir = dirs::home_dir().unwrap_or_default();
        let target = cx.prompt_for_new_path(&dir, Some(&format!("winfer-{}.winfer", chrono_date())));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = target.await else { return };
            let path = path.display().to_string();
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = this.settings.as_mut() {
                    s.busy = true;
                    s.export_result = None;
                }
                cx.notify();
            });
            let shown = path.clone();
            let result = tauri::async_runtime::spawn(async move { transfer::data_export(backend::app().clone(), path, Default::default(), secret).await }).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = this.settings.as_mut() {
                    s.busy = false;
                    s.export_result = Some(result.map(|_| shown));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn pick_import(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Import".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(path) = paths.first().map(|p| p.display().to_string()) else { return };
            let _ = this.update(cx, |this, cx| this.inspect_import(path, cx));
        })
        .detach();
    }

    /// Reads a picked or dropped export's manifest, to show what it holds before importing.
    fn inspect_import(&mut self, path: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let manifest = backend::background(move || transfer::data_import_inspect(target).map(|m| serde_json::to_value(m).unwrap_or_default())).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = this.settings.as_mut() {
                    match manifest {
                        Ok(m) => {
                            s.importing = Some((path, m));
                            s.import_result = None;
                        }
                        Err(err) => {
                            s.importing = None;
                            s.import_result = Some(Err(err));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn run_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ui) = self.settings.as_ref() else { return };
        let Some((path, manifest)) = ui.importing.clone() else { return };
        let typed = ui.import_passphrase.read(cx).value().to_string();
        let secret = (manifest["hasSecrets"] == true && !typed.is_empty()).then_some(typed);
        let detail = "Importing replaces this machine's settings, workspaces, plugins and usage history, then restarts winfer.".to_string();
        self.confirm("Import winfer data?", detail, "Import and restart", window, cx, move |this, cx| {
            cx.spawn(async move |this, cx| {
                let result = tauri::async_runtime::spawn(async move { transfer::data_import_stage(backend::app().clone(), path, secret).await }).await.map_err(|e| e.to_string()).and_then(|r| r);
                let _ = this.update(cx, |this, cx| match result {
                    Ok(_) => {
                        // An import staged for the next start mustn't be overwritten by this one.
                        this.suspend_saving = true;
                        cx.restart();
                    }
                    Err(err) => {
                        if let Some(s) = this.settings.as_mut() {
                            s.import_result = Some(Err(format!("Import failed: {err}")));
                        }
                        cx.notify();
                    }
                });
            })
            .detach();
            let _ = this;
        });
    }

    // ───────── Diagnostics ─────────

    pub(super) fn load_diagnostics(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let info = backend::background(|| {
                let mut info = serde_json::to_value(diagnostics::diagnostics_info(backend::app().clone())).unwrap_or_default();
                let dumps = minidumps(info["logDir"].as_str().unwrap_or_default());
                info["dumps"] = Value::Array(dumps);
                info
            })
            .await
            .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = this.settings.as_mut() {
                    s.verbose = info["verbose"] == true;
                    s.diagnostics = Some(info);
                    s.diagnostics_polled = Some(std::time::Instant::now());
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn diagnostics_tabs(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let info = self.settings.as_ref().and_then(|s| s.diagnostics.clone()).unwrap_or(Value::Null);
        let mono = crate::fonts::mono(cx);
        let logs = info["logBytes"].as_f64().unwrap_or(0.0) + info["crashBytes"].as_f64().unwrap_or(0.0);
        let dumps = info["dumps"].as_array().map_or(0, Vec::len);
        let tabs = vec![
            PageTab::new("overview", "Overview", IconName::Gauge).extra((info["previousUnexpectedExit"] == true).then(|| tab_dot(c.warning))),
            PageTab::new("logs", "Logs", IconName::ScrollText).extra((logs > 0.0).then(|| tab_count(c, &mono, bytes_label(logs)))),
            PageTab::new("crashes", "Crashes", IconName::Bug).extra((dumps > 0).then(|| tab_count(c, &mono, dumps))),
        ];
        self.page_tab_bar("diagnostics", "overview", tabs, c, cx)
    }

    pub(super) fn page_diagnostics(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let _ = window;
        // Memory moves: re-read every five seconds while the page is open.
        if self.settings.as_ref().is_some_and(|s| s.diagnostics_polled.is_none_or(|t| t.elapsed() > Duration::from_secs(5))) {
            if let Some(s) = self.settings.as_mut() {
                s.diagnostics_polled = Some(std::time::Instant::now());
            }
            self.load_diagnostics(cx);
        }
        let Some(info) = self.settings.as_ref().unwrap().diagnostics.clone() else {
            return div().child(empty_state(c, IconName::Activity, "Loading…"));
        };
        let body = match self.page_tab("diagnostics", "overview") {
            "logs" => self.diagnostics_logs(&info, c, cx),
            "crashes" => self.diagnostics_crashes(&info, c, cx),
            _ => self.diagnostics_overview(&info, c, cx),
        };
        body.pt_3()
    }

    fn diagnostics_overview(&mut self, info: &Value, c: Colors, cx: &mut Context<Self>) -> Div {
        let mono = crate::fonts::mono(cx);
        let num = |k: &str| info[k].as_f64().unwrap_or(0.0);
        let mem = |k: &str| info["memory"][k].as_f64().unwrap_or(0.0);
        let mbs = |b: f64| format!("{:.0}", b / 1048576.0);
        let gb = |b: f64| format!("{:.1}", b / 1073741824.0);
        let crash = info["crashTail"].as_str().unwrap_or("").to_string();
        let summary = format!(
            "winfer {} · {} {} · up {} min\nmemory: {} MB now, {} MB session peak\npanes: {} terminal sessions, {} scrollback\nlogs: {} ({} log, {} crash log)\n{}",
            info["version"].as_str().unwrap_or_default(), info["os"].as_str().unwrap_or_default(), info["arch"].as_str().unwrap_or_default(), (num("uptimeSecs") / 60.0).floor(),
            mbs(mem("rssBytes")), mbs(num("sessionPeakRssBytes")), num("ptySessions"), bytes_label(num("scrollbackBytes")),
            info["logDir"].as_str().unwrap_or_default(), bytes_label(num("logBytes")), bytes_label(num("crashBytes")),
            if info["previousUnexpectedExit"] == true { "previous run ended without a clean exit (crash or OOM kill)" } else { "previous run exited cleanly" },
        );
        let copy_text = format!("{summary}\n\ncrash log tail:\n{}", if crash.is_empty() { "(empty)" } else { &crash });

        let unclean = (info["previousUnexpectedExit"] == true).then(|| {
            div().flex().items_start().gap_2p5().px_3p5().py_3().rounded(px(10.0)).bg(alpha(c.warning, 0.12))
                .child(Icon::new(IconName::TriangleAlert).size(px(16.0)).text_color(c.warning).mt(px(1.0)))
                .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                    .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.warning).child("The last run didn't exit cleanly"))
                    .child(div().text_size(px(12.0)).text_color(c.muted).child("Probably a crash or an out-of-memory kill. Its last memory samples are at the end of winfer.log; export a report and attach it to a bug.")))
                .when(!crash.is_empty(), |d| d.child(button(c, "diag-see-crash", None, "See crash", Look::Plain, true).on_click(cx.listener(|this, _, _, cx| this.set_page_tab("diagnostics", "crashes", cx)))))
        });

        let unit_tile = |label: &str, icon: IconName, value: String, unit: &str, detail: String| {
            panel(c).flex_1().min_w_0().gap_1().px_3p5().py_3()
                .child(div().flex().items_center().gap_1p5().text_size(px(11.5)).text_color(c.subtle).child(Icon::new(icon).size(px(12.0))).child(label.to_string()))
                .child(div().flex().items_baseline().gap_1()
                    .child(div().text_size(px(20.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(value))
                    .child(div().text_size(px(12.0)).text_color(c.subtle).child(unit.to_string())))
                .child(div().truncate().text_size(px(11.5)).text_color(c.subtle).child(detail))
        };
        let (avail, total) = (mem("systemAvailableBytes"), mem("systemTotalBytes"));
        let free_tile = panel(c).flex_1().min_w_0().gap_1().px_3p5().py_3()
            .child(div().flex().items_center().gap_1p5().text_size(px(11.5)).text_color(c.subtle).child(Icon::new(IconName::Cpu).size(px(12.0))).child("System free"))
            .child(div().flex().items_baseline().gap_1()
                .child(div().text_size(px(20.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(gb(avail)))
                .child(div().text_size(px(12.0)).text_color(c.subtle).child(format!("of {:.0} GB", total / 1073741824.0))))
            .child(div().pt_1().child(meter(c, if total > 0.0 { (avail / total) as f32 } else { 0.0 }, c.success, 5.0)));
        let peak_bytes = num("sessionPeakRssBytes").max(mem("rssBytes"));
        let tiles = div().flex().gap_2p5()
            .child(unit_tile("winfer now", IconName::MemoryStick, mbs(mem("rssBytes")), "MB", "resident memory".into()))
            .child(unit_tile("Session peak", IconName::TrendingUp, mbs(peak_bytes), "MB", "since winfer started".into()))
            .child(free_tile)
            .child(unit_tile("Terminals", IconName::SquareTerminal, format!("{}", num("ptySessions")), "panes", format!("{} scrollback", bytes_label(num("scrollbackBytes")))));

        let recent: Vec<String> = info["recentMemory"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
        let values = sample_values(&recent);
        let peak = (peak_bytes / 1048576.0) as f32;
        let chart = memory_chart(c, mono.clone(), values, peak);

        let facts = div().flex().items_center().gap_4().px_3p5().py_2p5().rounded(px(10.0)).border_1().border_color(c.edge).bg(c.panel).text_size(px(12.0)).text_color(c.muted)
            .child(div().flex().items_center().gap_1p5().child(Icon::new(IconName::Tag).size(px(13.0)).text_color(c.subtle)).child("winfer").child(div().font_family(mono.clone()).text_color(c.fg).child(info["version"].as_str().unwrap_or_default().to_string())))
            .child(div().flex().items_center().gap_1p5().child(Icon::new(IconName::Monitor).size(px(13.0)).text_color(c.subtle)).child(format!("{} · {}", os_name(info["os"].as_str().unwrap_or_default()), info["arch"].as_str().unwrap_or_default())))
            .child(div().flex().items_center().gap_1p5().child(Icon::new(IconName::Clock).size(px(13.0)).text_color(c.subtle)).child(format!("up {}", uptime(num("uptimeSecs")))))
            .child(div().flex_1())
            .child(div().flex().items_center().gap_1p5().text_color(c.subtle).child(Icon::new(IconName::RefreshCw).size(px(12.0))).child("live, every 5 s"));

        let action = |id: &'static str, icon: IconName, title: &str, sub: &str| {
            div().id(id).flex_1().min_w_0().flex().items_center().gap_3().px_3p5().py_3().rounded(px(10.0)).border_1().border_color(c.edge).bg(c.panel).cursor_pointer()
                .hover(move |d| d.bg(c.hover))
                .child(icon_tile(c.accent, icon, 30.0))
                .child(small_label(c, title.to_string(), sub.to_string()))
                .child(Icon::new(IconName::ChevronRight).size(px(14.0)).text_color(c.subtle))
        };
        let report = div().flex().gap_2p5()
            .child(action("diag-copy", IconName::Copy, "Copy summary", "Version, memory and the crash-log tail, to paste into a bug report").on_click(cx.listener(move |this, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                this.toast("Diagnostics summary copied", cx);
            })))
            .child(action("diag-export", IconName::FileArchive, "Export report…", "A zip with the summary, both logs and the memory timeline").on_click(cx.listener(|this, _, _, cx| this.export_diagnostics(cx))));
        let result = self.settings.as_ref().and_then(|s| s.diagnostics_result.clone()).map(|r| {
            let actions = r.as_ref().ok().map(|path| {
                let path = path.clone();
                button(c, "diag-reveal", Some(IconName::FolderOpen), "Show in Finder", Look::Plain, true).on_click(cx.listener(move |_, _, _, _| sidebar::reveal_path(&path))).into_any_element()
            });
            let shown = r.map(|p| format!("Exported to {p}")).map_err(|e| format!("Export failed: {e}"));
            result_banner(c, &shown, actions)
        });
        div().flex().flex_col().gap_4()
            .children(unclean)
            .child(tiles)
            .child(chart)
            .child(facts)
            .child(section(c, "Report a problem", Some("no settings, chats or secrets are included".into())))
            .child(report)
            .children(result)
            .child(section(c, "Usage data", Some("anonymous".into())))
            .child(self.usage_data_card(c, cx))
    }

    /// Opt-out for the anonymous usage data (telemetry.rs).
    fn usage_data_card(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let available = crate::telemetry::available();
        let on = available && crate::telemetry::enabled();
        let sub = if available { "Helps improve winfer. Never your prompts, code or files." } else { "This build doesn't send usage data." };
        let info = div().id("usage-data-info").size(px(26.0)).flex_shrink_0().flex().items_center().justify_center().rounded_md().cursor_pointer()
            .text_color(c.subtle).hover(move |d| d.bg(c.hover).text_color(c.fg))
            .tooltip(|window, cx| Tooltip::new("What's collected").build(window, cx))
            .on_click(cx.listener(|this, _, _, cx| {
                this.usage_info_open = true;
                cx.notify();
            }))
            .child(Icon::new(IconName::Info).size(px(15.0)));
        let card = panel(c).flex_row().items_center().gap_3().px_3p5().py_3()
            .child(icon_tile(c.muted, IconName::ChartColumn, 28.0))
            .child(small_label(c, "Share anonymous usage data", sub))
            .child(info);
        if !available {
            return card;
        }
        card.child(self.toggle("usage-data", on, c, cx, |_, cx| {
            crate::telemetry::set_enabled(!crate::telemetry::enabled());
            cx.notify();
        }))
    }

    fn diagnostics_logs(&mut self, info: &Value, c: Colors, cx: &mut Context<Self>) -> Div {
        let mono = crate::fonts::mono(cx);
        let num = |k: &str| info[k].as_f64().unwrap_or(0.0);
        let log_dir = info["logDir"].as_str().unwrap_or_default().to_string();
        let reveal_dir = log_dir.clone();
        let dumps: Vec<Value> = info["dumps"].as_array().cloned().unwrap_or_default();
        let dump_bytes: f64 = dumps.iter().filter_map(|d| d["bytes"].as_f64()).sum();
        let files = [
            (IconName::FileText, "winfer.log".to_string(), "The app log · rotates at 5 MB", num("logBytes"), c.accent),
            (IconName::Bug, "crash.log".to_string(), "Panics with backtraces", num("crashBytes"), c.warning),
            (IconName::FileArchive, format!("{} minidump{}", dumps.len(), if dumps.len() == 1 { "" } else { "s" }), "Native crash dumps · see Crashes", dump_bytes, c.danger),
        ];
        let biggest = files.iter().map(|f| f.3).fold(1.0, f64::max);
        let mut list = panel(c);
        for (i, (icon, name, sub, bytes, color)) in files.into_iter().enumerate() {
            list = list.child(div().flex().items_center().gap_3().px_3p5().py_2p5().when(i > 0, |d| d.border_t_1().border_color(c.border))
                .child(icon_tile(c.muted, icon, 28.0))
                .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                    .child(div().font_family(mono.clone()).text_size(px(12.5)).text_color(c.fg).child(name))
                    .child(div().text_size(px(11.5)).text_color(c.subtle).child(sub)))
                .child(div().w(px(170.0)).child(meter(c, (bytes / biggest) as f32, color, 5.0)))
                .child(div().w(px(64.0)).text_right().font_family(mono.clone()).text_size(px(11.5)).text_color(c.muted).child(bytes_label(bytes))));
        }
        let folder = panel(c).flex_row().items_center().gap_3().px_3().py_2p5()
            .child(icon_tile(c.muted, IconName::Folder, 28.0))
            .child(div().flex_1().min_w_0().truncate().px_2p5().py_1p5().rounded_md().bg(c.app).font_family(mono).text_size(px(12.0)).text_color(c.muted).child(log_dir))
            .child(button(c, "diag-open", Some(IconName::FolderOpen), "Show in Finder", Look::Plain, true).on_click(cx.listener(move |_, _, _, _| sidebar::reveal_path(&reveal_dir))));
        let verbose = self.settings.as_ref().is_some_and(|s| s.verbose);
        let verbose_toggle = self.toggle("verbose-log", verbose, c, cx, |this, cx| {
            if let Some(s) = this.settings.as_mut() {
                s.verbose = !s.verbose;
                diagnostics::diagnostics_set_verbose(s.verbose);
            }
            this.load_diagnostics(cx);
        });
        let verbose_card = panel(c).flex_row().items_center().gap_3().px_3p5().py_3()
            .child(icon_tile(c.muted, IconName::ListFilter, 28.0))
            .child(small_label(c, "Verbose logging", "Also records debug lines: more detail, but the log grows faster. Kept across restarts."))
            .child(verbose_toggle);
        let clear = div().flex().items_center().gap_3().px_3p5().py_3().rounded(px(10.0)).border_1().border_color(alpha(c.danger, 0.35))
            .child(icon_tile(c.danger, IconName::Trash, 28.0))
            .child(small_label(c, "Clear logs", "Deletes winfer.log and crash.log. Minidumps are kept."))
            .child(button(c, "diag-clear", None, "Clear…", Look::Danger, true).on_click(cx.listener(|this, _, window, cx| {
                this.confirm("Clear logs?", "Delete the current log and crash log? Minidumps are kept.".to_string(), "Clear", window, cx, |this, cx| {
                    let _ = diagnostics::diagnostics_clear();
                    this.load_diagnostics(cx);
                });
            })));
        div().flex().flex_col().gap_4().child(folder).child(list).child(verbose_card).child(clear)
    }

    fn diagnostics_crashes(&mut self, info: &Value, c: Colors, cx: &mut Context<Self>) -> Div {
        let mono = crate::fonts::mono(cx);
        let crash = info["crashTail"].as_str().unwrap_or("").to_string();
        let crash_file = std::path::Path::new(info["logDir"].as_str().unwrap_or_default()).join("crash.log");
        let dumps: Vec<Value> = info["dumps"].as_array().cloned().unwrap_or_default();
        let latest = if crash.trim().is_empty() {
            panel(c).child(empty_state(c, IconName::ShieldCheck, "No crashes recorded. crash.log is empty."))
        } else {
            let copy = crash.clone();
            let file = crash_file.clone();
            let mut lines = div().id("crash-tail").max_h(px(260.0)).overflow_y_scroll().flex().flex_col().px_3p5().py_3().bg(c.app).rounded_b(px(10.0)).font_family(mono.clone()).text_size(px(11.0));
            for line in crash.lines() {
                let color = if line.contains("panicked") { c.danger } else if line.trim_start().starts_with("at ") || line.contains("stack backtrace") { c.subtle } else if line.contains("::") { c.fg } else { c.muted };
                lines = lines.child(div().text_color(color).child(line.to_string()));
            }
            panel(c)
                .child(div().flex().items_center().gap_3().px_3p5().py_2p5()
                    .child(icon_tile(c.danger, IconName::Bug, 28.0))
                    .child(small_label(c, "Latest crash", "The end of crash.log · full backtraces are in the file and the report"))
                    .child(button(c, "crash-copy", Some(IconName::Copy), "Copy", Look::Plain, true).on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                        this.toast("Crash log copied", cx);
                    })))
                    .child(button(c, "crash-open", Some(IconName::ExternalLink), "Open file", Look::Plain, true).on_click(cx.listener(move |_, _, _, cx| cx.open_with_system(&file)))))
                .child(div().border_t_1().border_color(c.border).child(lines))
        };
        let mut body = div().flex().flex_col().gap_4().child(latest);
        body = body.child(section(c, "Minidumps", (!dumps.is_empty()).then(|| "attach the newest .dmp to a crash report".to_string())));
        if dumps.is_empty() {
            body = body.child(panel(c).child(empty_state(c, IconName::FileArchive, "No minidumps. winfer keeps them in the log folder when it crashes natively.")));
        } else {
            let mut list = panel(c);
            for (i, d) in dumps.iter().enumerate() {
                let name = d["name"].as_str().unwrap_or_default().to_string();
                let path = std::path::Path::new(info["logDir"].as_str().unwrap_or_default()).join(&name).display().to_string();
                list = list.child(div().flex().items_center().gap_3().px_3p5().py_2p5().when(i > 0, |d| d.border_t_1().border_color(c.border))
                    .child(icon_tile(c.muted, IconName::FileArchive, 28.0))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                        .child(div().truncate().font_family(mono.clone()).text_size(px(12.5)).text_color(c.fg).child(name))
                        .child(div().text_size(px(11.5)).text_color(c.subtle).child(format!("{} · {}", date_of(d["modified"].as_i64().unwrap_or(0)), bytes_label(d["bytes"].as_f64().unwrap_or(0.0))))))
                    .when(i == 0, |d| d.child(badge(c.warning, "newest")))
                    .child(button(c, SharedString::from(format!("dump-show-{i}")), Some(IconName::FolderOpen), "Show", Look::Plain, true).on_click(cx.listener(move |_, _, _, _| sidebar::reveal_path(&path)))));
            }
            body = body.child(list);
        }
        body
    }

    fn export_diagnostics(&mut self, cx: &mut Context<Self>) {
        let dir = dirs::home_dir().unwrap_or_default();
        let target = cx.prompt_for_new_path(&dir, Some(&format!("winfer-diagnostics-{}.zip", chrono_date())));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = target.await else { return };
            let path = path.display().to_string();
            let shown = path.clone();
            let result = tauri::async_runtime::spawn(async move { diagnostics::diagnostics_export(backend::app().clone(), path).await }).await.map_err(|e| e.to_string()).and_then(|r| r).map(|_| shown);
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = this.settings.as_mut() {
                    s.diagnostics_result = Some(result);
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// winfer's memory over the last samples as a line with a soft fill, and the session peak as a
/// dashed line above it.
fn memory_chart(c: Colors, mono: SharedString, values: Vec<f32>, peak: f32) -> Div {
    let legend = div().flex().items_center().gap_3().text_size(px(11.5)).text_color(c.subtle)
        .child(div().flex().items_center().gap_1p5().child(div().w(px(12.0)).h(px(2.0)).rounded_full().bg(c.accent)).child("winfer"))
        .child(div().flex().items_center().gap_1p5().child(div().w(px(12.0)).h(px(2.0)).rounded_full().bg(alpha(c.warning, 0.8))).child(format!("session peak {peak:.0} MB")))
        .child("sampled every 5 s");
    let head = div().flex().items_center().gap_2().child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child("Memory, last 2 minutes")).child(div().flex_1()).child(legend);
    if values.len() < 2 {
        return panel(c).gap_2().px_3p5().py_3().child(head).child(div().h(px(90.0)).flex().items_center().justify_center().text_size(px(12.0)).text_color(c.subtle).child("Collecting samples…"));
    }
    let span_secs = (values.len() - 1) * 5;
    let lo = values.iter().cloned().fold(f32::MAX, f32::min);
    let hi = values.iter().cloned().fold(peak, f32::max);
    let (min, max) = ((lo - (hi - lo).max(20.0) * 0.6).max(0.0), hi + (hi - lo).max(20.0) * 0.12);
    let (line, fill, peak_color, grid) = (c.accent, alpha(c.accent, 0.16), alpha(c.warning, 0.8), alpha(c.border, 0.8));
    let plot = canvas(|_, _, _| {}, move |bounds, _, window, _| {
        let n = values.len() as f32 - 1.0;
        let at = |i: usize, v: f32| point(bounds.origin.x + bounds.size.width * (i as f32 / n), bounds.origin.y + bounds.size.height * (1.0 - (v - min) / (max - min)));
        for k in 1..4 {
            let y = bounds.origin.y + bounds.size.height * (k as f32 / 4.0);
            window.paint_quad(fill_quad(Bounds::new(point(bounds.origin.x, y), size(bounds.size.width, px(1.0))), grid));
        }
        let mut area = PathBuilder::fill();
        area.move_to(point(bounds.origin.x, bounds.bottom()));
        for (i, v) in values.iter().enumerate() {
            area.line_to(at(i, *v));
        }
        area.line_to(point(bounds.right(), bounds.bottom()));
        area.close();
        if let Ok(path) = area.build() {
            window.paint_path(path, fill);
        }
        let mut stroke = PathBuilder::stroke(px(1.75));
        for (i, v) in values.iter().enumerate() {
            if i == 0 { stroke.move_to(at(i, *v)) } else { stroke.line_to(at(i, *v)) }
        }
        if let Ok(path) = stroke.build() {
            window.paint_path(path, line);
        }
        // The session peak, dashed.
        let y = at(0, peak).y;
        let mut x = bounds.origin.x;
        while x < bounds.right() {
            window.paint_quad(fill_quad(Bounds::new(point(x, y), size(px(6.0).min(bounds.right() - x), px(1.5))), peak_color));
            x += px(10.0);
        }
        let last = at(values.len() - 1, *values.last().unwrap_or(&0.0));
        window.paint_quad(fill_quad(Bounds::new(point(last.x - px(3.5), last.y - px(3.5)), size(px(7.0), px(7.0))), line).corner_radii(px(3.5)));
    })
    .size_full();
    let axis = div().flex().justify_between().font_family(mono).text_size(px(10.5)).text_color(c.subtle)
        .child(format!("-{} min", (span_secs as f32 / 60.0).max(0.1).round().max(1.0)))
        .child("now");
    panel(c).gap_2().px_3p5().py_3().child(head).child(div().h(px(96.0)).child(plot)).child(axis)
}

fn fill_quad(bounds: Bounds<Pixels>, color: Hsla) -> PaintQuad {
    gpui_kit::fill(bounds, color)
}

#[cfg(test)]
mod tests {
    use super::{bytes_label, date_of, sample_values, uptime};

    #[test]
    fn diagnostics_formatting() {
        assert_eq!(sample_values(&["2026-11-22T10:15:03Z 412MB".into(), "2026-11-22T10:15:08Z 415MB".into()]), vec![412.0, 415.0]);
        assert_eq!(bytes_label(38.0 * 1024.0), "38 KB");
        assert_eq!(bytes_label(4.2 * 1048576.0), "4.2 MB");
        assert_eq!(bytes_label(538.0 * 1048576.0), "538 MB");
        assert_eq!(uptime(8040.0), "2 h 14 min");
        assert_eq!(date_of(1_795_305_600), "2026-11-22");
        assert_eq!(date_of(1_795_392_000), "2026-11-23");
    }
}

/// What the usage data holds, in groups (telemetry.rs sends exactly this).
const USAGE_DATA: &[(IconName, &str, &[&str])] = &[
    (IconName::Monitor, "App and session", &[
        "A random install ID, not tied to you or this computer",
        "App version, operating system and architecture",
        "Startup time and session length",
        "Memory use every hour and after 1, 4, 8, 24 and 72 hours",
    ]),
    (IconName::LayoutGrid, "Features", &[
        "Agent panes opened, by provider and reasoning effort",
        "Commands used, by name and count",
        "Worktrees created; plugins switched on or off (store plugins by name)",
        "How many workspaces and panes are open",
        "Which settings differ from the defaults (values only for choices like the theme)",
    ]),
    (IconName::Bot, "Agents and tokens", &[
        "Tokens per provider and model, per hour",
        "Agent tool calls per tool: count, failures, average time",
        "winfer code turns per reasoning effort",
        "Live voice: model, tokens, response time and tool names",
    ]),
    (IconName::Bug, "Reliability", &[
        "Crashes: the source file and line, never the message",
        "Whether the previous run ended unexpectedly",
    ]),
];

const USAGE_DATA_NEVER: &str = "Prompts, transcripts, code, terminal output, file paths, folder and repository names, URLs, messages, or your location.";

impl Root {
    /// The list of usage data sent, over everything (Settings → Diagnostics → the info button).
    pub(super) fn render_usage_info(&mut self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.usage_info_open {
            return None;
        }
        let c = *c;
        let close = |this: &mut Self, cx: &mut Context<Self>| {
            this.usage_info_open = false;
            cx.notify();
        };
        let mut body = div().id("usage-info-body").flex().flex_col().gap_4().overflow_y_scroll().px_5().py_4();
        for (icon, title, items) in USAGE_DATA {
            let mut list = div().flex().flex_col().gap_1p5().pl(px(24.0));
            for item in *items {
                list = list.child(div().flex().items_start().gap_2().text_size(px(12.5)).text_color(c.muted)
                    .child(div().mt(px(7.0)).size(px(4.0)).flex_shrink_0().rounded_full().bg(c.subtle))
                    .child(div().flex_1().min_w_0().child(*item)));
            }
            body = body.child(div().flex().flex_col().gap_2()
                .child(div().flex().items_center().gap_2().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg)
                    .child(Icon::new(*icon).size(px(15.0)).text_color(c.subtle)).child(*title))
                .child(list));
        }
        body = body.child(div().flex().items_start().gap_2p5().px_3().py_2p5().rounded(px(9.0)).bg(alpha(c.success, 0.1))
            .child(div().mt(px(1.0)).child(Icon::new(IconName::ShieldCheck).size(px(15.0)).text_color(c.success)))
            .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child("Never sent"))
                .child(div().text_size(px(12.0)).text_color(c.muted).child(USAGE_DATA_NEVER))));
        let footer = div().flex().items_center().gap_3().px_5().py_3().border_t_1().border_color(c.border)
            .child(div().flex_1().text_size(px(11.5)).text_color(c.subtle).child("Turn it off with the switch, or set DO_NOT_TRACK=1."))
            .child(div().id("usage-info-ok").rounded_md().bg(c.accent).text_color(c.on_accent).px_3().py_1().text_size(px(12.0)).cursor_pointer().hover(|d| d.opacity(0.9))
                .on_click(cx.listener(move |this, _, _, cx| close(this, cx))).child("Got it"));
        Some(
            div().id("usage-info-backdrop").absolute().size_full().flex().items_center().justify_center().bg(hsla(0.0, 0.0, 0.0, 0.4)).occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| close(this, cx)))
                .child(
                    div().id("usage-info").w(px(500.0)).max_h(relative(0.8)).flex().flex_col().rounded_xl().border_1().border_color(c.edge).bg(c.raised).shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().flex().items_start().gap_3().px_5().py_3().border_b_1().border_color(c.border)
                            .child(div().mt(px(2.0)).child(Icon::new(IconName::ChartColumn).size(px(16.0)).text_color(c.accent)))
                            .child(div().flex_1().min_w_0().flex().flex_col()
                                .child(div().text_size(px(14.0)).text_color(c.fg).child("Anonymous usage data"))
                                .child(div().text_size(px(12.0)).text_color(c.subtle).child("What winfer sends to help improve the app. Nothing here identifies you."))))
                        .child(body)
                        .child(footer),
                )
                .into_any_element(),
        )
    }
}
