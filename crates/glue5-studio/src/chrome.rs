//! Around the work: title bar, activity bar, status bar, the phone's navigation.

use leptos::prelude::*;
use serde_json::{Value, json};

use crate::icons;
use crate::net::request;
use crate::state::*;

#[component]
pub(crate) fn TitleBar(s: S) -> impl IntoView {
    let search = move |q: String| {
        s.query.set(q.clone());
        if q.trim().len() < 2 {
            s.found.set(Vec::new());
            return;
        }
        request("natives.search", json!({ "query": q, "limit": 12 }), move |r| {
            if let Ok(Value::Array(list)) = r {
                s.found.set(list);
            }
        });
    };
    view! {
        <header class="titlebar">
            <div class="brand">
                <img src="/logo.png" alt=""/><b>"GLUE 5"</b><span>"STUDIO"</span>
            </div>
            <div class="palette-wrap">
                <label class="palette">
                    <span class="ico" inner_html=icons::SEARCH></span>
                    <input placeholder="Search natives" spellcheck="false"
                        prop:value=move || s.query.get()
                        on:focus=move |_| s.palette.set(true)
                        on:blur=move |_| set_timeout(move || s.palette.set(false), std::time::Duration::from_millis(200))
                        on:input=move |ev| search(event_target_value(&ev))
                        on:keydown=move |ev| if ev.key() == "Escape" { s.query.set(String::new()); s.found.set(Vec::new()); } />
                    <kbd>"natives"</kbd>
                </label>
                <Show when=move || s.palette.get() && s.found.with(|f| !f.is_empty())>
                    <div class="palette-results">
                        {move || s.found.get().into_iter().map(|n| {
                            let name = n["name"].as_str().unwrap_or("").to_string();
                            let insert = format!("{name}(");
                            view! {
                                <button class="result" on:mousedown=move |_| { with_editor(|e| e.insert(&insert)); }>
                                    <span class="sig">{n["signature"].as_str().unwrap_or("").to_string()}</span>
                                    <small>{n["native"].as_str().unwrap_or("").to_string()}</small>
                                </button>
                            }
                        }).collect_view()}
                    </div>
                </Show>
            </div>
            <div class="tb-right">
                <div class=move || if s.connected.get() { "device" } else { "device off" }>
                    <span class="dot"></span>
                    {move || s.hello.with(|h| h["device"].as_str().unwrap_or("Game").to_string())}
                    <small>{move || if s.connected.get() { " · connected" } else { " · reconnecting…" }}</small>
                </div>
            </div>
        </header>
    }
}

#[component]
pub(crate) fn Activity(s: S) -> impl IntoView {
    let errors = move || s.diags.with(|d| d.values().flatten().filter(|d| d.severity == "error").count());
    let item = move |side: Option<Side>, panel: Option<Panel>, title: &'static str, icon: &'static str| {
        let on = move || match (side, panel) {
            (Some(sd), _) => s.side.get() == sd,
            (_, Some(p)) => s.panel.get() == p,
            _ => false,
        };
        view! {
            <button class=move || if on() { "act on" } else { "act" } title=title
                on:click=move |_| {
                    if let Some(sd) = side {
                        if s.side.get_untracked() == sd { s.drawer.update(|d| *d = !*d) } else { s.drawer.set(true) }
                        s.side.set(sd);
                    }
                    if let Some(p) = panel { s.panel.set(p) }
                }>
                <span class="ico" inner_html=icon></span>
                {(panel == Some(Panel::Problems)).then(|| view! {
                    <Show when=move || errors() != 0><span class="badge">{errors}</span></Show>
                })}
            </button>
        }
    };
    view! {
        <nav class="activity">
            {item(Some(Side::Mods), None, "Mods", icons::FOLDER)}
            {item(Some(Side::Natives), None, "Natives", icons::BOOK)}
            {item(None, Some(Panel::Problems), "Problems", icons::WARNING)}
            {item(None, Some(Panel::Console), "Console", icons::TERMINAL)}
        </nav>
    }
}

#[component]
pub(crate) fn StatusBar(s: S) -> impl IntoView {
    let errors = move || s.diags.with(|d| d.values().flatten().filter(|d| d.severity == "error").count());
    view! {
        <footer class="status">
            <span class="it"><span class=move || if s.connected.get() { "dot" } else { "dot off" }></span>
                <b>{move || if s.connected.get() { "Connected" } else { "Offline" }}</b></span>
            <span class="it hide-t">{move || s.hello.with(|h| format!("build {}", h["build"].as_str().unwrap_or("")))}</span>
            <Show when=move || errors() != 0>
                <span class="it errn" on:click=move |_| s.panel.set(Panel::Problems)>{move || format!("● {} error{}", errors(), if errors() == 1 { "" } else { "s" })}</span>
            </Show>
            <Show when=move || s.hello.with(|h| h["tainted"] == true)>
                <span class="it errn">"A native failed: restart the game soon"</span>
            </Show>
            <span class="grow"></span>
            <span class="it">{move || { let (l, c) = s.cursor.get(); format!("Ln {l}, Col {c}") }}</span>
            <span class="it hide-t">"Luau"</span>
            <span class="it">{move || {
                let mods = s.mods.get();
                let running: Vec<_> = mods.iter().filter(|m| m.state == "running").collect();
                format!("{} running · {:.1} ms/frame", running.len(), running.iter().map(|m| m.cpu_ms).fold(0.0, |a, b| a + b))
            }}</span>
        </footer>
    }
}

#[component]
pub(crate) fn PhoneNav(s: S) -> impl IntoView {
    let errors = move || s.diags.with(|d| d.values().flatten().filter(|d| d.severity == "error").count());
    let item = move |screen: Screen, label: &'static str, icon: &'static str| {
        view! {
            <button class=move || if s.screen.get() == screen { "on" } else { "" }
                on:click=move |_| {
                    s.screen.set(screen);
                    match screen {
                        Screen::Console => s.panel.set(Panel::Console),
                        Screen::Problems => s.panel.set(Panel::Problems),
                        Screen::Menus => s.panel.set(Panel::Menus),
                        _ => {}
                    }
                }>
                <span class="ico" inner_html=icon></span>{label}
                {(screen == Screen::Problems).then(|| view! { <Show when=move || errors() != 0><span class="badge">{errors}</span></Show> })}
            </button>
        }
    };
    view! {
        <nav class="phone-nav">
            {item(Screen::Mods, "Mods", icons::FOLDER)}
            {item(Screen::Editor, "Editor", icons::CODE)}
            {item(Screen::Console, "Console", icons::TERMINAL)}
            {item(Screen::Problems, "Problems", icons::WARNING)}
            {item(Screen::Menus, "Menus", icons::SLIDERS)}
        </nav>
    }
}
