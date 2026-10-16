//! Friends: the player's friend code, asking someone with theirs, the
//! requests to answer, and the friends, with what they play and a way to
//! join them in GTA Online.
use glue5_runtime::friends::{self, AddError, FriendInfo, Playing, Snapshot};

use super::pages::sheet;
use super::widgets::{Control, action_button, setting, text_field};
use super::{Action, WARN, theme};
use egui::RichText;

/// What the page keeps between frames.
#[derive(Default)]
pub struct FriendsPage {
    /// The code being typed.
    code: String,
    /// What came of the last thing done (and whether it went wrong).
    message: Option<(String, bool)>,
    /// The friend whose removal waits for a second press.
    removing: Option<String>,
}

/// What the game lets the page do: `started`, a game runs; `online`, it is
/// GTA Online (friends are invited to its session, joined from it).
pub struct Context {
    pub started: bool,
    pub online: bool,
}

/// True when closed.
pub(super) fn friends_page(ctx: &egui::Context, page: &mut FriendsPage, game: Context, actions: &mut Vec<Action>) -> bool {
    // What the friends do comes from the network: the page follows it.
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
    let snapshot = friends::snapshot();
    sheet(ctx, "friends-window", "FRIENDS", |ui| {
        let scale = theme::density(ui);
        let muted = |ui: &mut egui::Ui, text: &str| {
            ui.label(RichText::new(text).size(12.0 * scale).color(theme::MUTED));
        };
        your_code(ui, &snapshot, page);
        ask(ui, page);
        if let Some((text, wrong)) = &page.message {
            ui.label(RichText::new(text).size(12.0 * scale).color(if *wrong { WARN } else { theme::MUTED }));
        }
        if !snapshot.requests.is_empty() {
            heading(ui, "ASKING YOU");
            for (key, name) in &snapshot.requests {
                ui.push_id(key, |ui| {
                    setting(ui, name, Control::Action, Some("Wants to be your friend"), |ui| {
                        ui.horizontal(|ui| {
                            if action_button(ui, "Accept").clicked() {
                                friends::answer(key, true);
                                page.message = Some((format!("{name} is your friend now."), false));
                            }
                            if action_button(ui, "Decline").clicked() {
                                friends::answer(key, false);
                            }
                        });
                    });
                });
            }
        }
        if !snapshot.asked.is_empty() {
            heading(ui, "ASKED");
            for a in &snapshot.asked {
                let state = match (a.declined, a.reached) {
                    (true, _) => "Said no",
                    (false, true) => "Reached: waiting for an answer",
                    (false, false) => "Looking: they need Glue 5 open",
                };
                ui.push_id(&a.code, |ui| {
                    setting(ui, &a.code, Control::Action, Some(state), |ui| {
                        if action_button(ui, if a.declined { "Dismiss" } else { "Cancel" }).clicked() {
                            friends::cancel(&a.code);
                        }
                    });
                });
            }
        }
        heading(ui, "FRIENDS");
        if snapshot.friends.is_empty() {
            muted(ui, "No friends yet. Give your code to the players you want as friends, or ask them with theirs.");
        }
        for f in &snapshot.friends {
            friend_row(ui, f, page, &game, actions);
        }
        ui.add_space(4.0 * scale);
        muted(
            ui,
            "Friends find each other directly, with no server: they see when you play and can join you. \
             What you say to each other is encrypted; like any player you meet online, a friend sees your \
             Internet address.",
        );
    })
}

fn heading(ui: &mut egui::Ui, text: &str) {
    let scale = theme::density(ui);
    ui.add_space(4.0 * scale);
    ui.label(theme::heading(text, 17.0 * scale).color(theme::MUTED));
}

fn your_code(ui: &mut egui::Ui, snapshot: &Snapshot, page: &mut FriendsPage) {
    let code = snapshot.code.clone().or_else(friends::my_code).unwrap_or_default();
    let what = "Your friend code: with it a player can ask to be your friend";
    setting(ui, &code, Control::Action, Some(what), |ui| {
        ui.horizontal(|ui| {
            if action_button(ui, "New").on_hover_text("A new code: the old one stops working").clicked() {
                friends::new_code();
                page.message = Some(("A new code: the old one no longer reaches you.".into(), false));
            }
            if action_button(ui, "Copy").clicked() {
                ui.ctx().copy_text(code.clone());
                page.message = Some(("Code copied.".into(), false));
            }
        });
    });
}

fn ask(ui: &mut egui::Ui, page: &mut FriendsPage) {
    let scale = theme::density(ui);
    ui.horizontal(|ui| {
        let button = 92.0 * scale;
        let field = ui.available_width() - button - ui.spacing().item_spacing.x;
        let typed = ui
            .allocate_ui(egui::vec2(field, 44.0 * scale), |ui| {
                text_field(ui, "Code", &mut page.code, "Your friend's code", 24)
            })
            .inner;
        let enter = typed.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let send = egui::Button::new(RichText::new("Ask").size(14.0 * scale))
            .corner_radius((12.0 * scale) as u8)
            .min_size(egui::vec2(button, 44.0 * scale));
        let sent = super::widgets::focus(ui.add_enabled(!page.code.trim().is_empty(), send));
        if sent.clicked() || enter && !page.code.trim().is_empty() {
            page.message = Some(match friends::ask(&page.code) {
                Ok(()) => {
                    page.code.clear();
                    ("Asked: they will see it once they open Glue 5.".into(), false)
                }
                Err(AddError::Invalid) => ("That is not a friend code: check it again.".into(), true),
                Err(AddError::Own) => ("That is your own code.".into(), true),
                Err(AddError::Asked) => ("Asked already.".into(), true),
            });
        }
    });
}

fn friend_row(ui: &mut egui::Ui, f: &FriendInfo, page: &mut FriendsPage, game: &Context, actions: &mut Vec<Action>) {
    let status = match (f.playing, f.lobby.as_deref()) {
        _ if f.invited => "Invites you to their session",
        (None, _) => "Offline",
        (Some(Playing::Menu), _) => "Online, not playing",
        (Some(Playing::Story), _) => "Playing story mode",
        (Some(Playing::Online), Some("")) => "In GTA Online, public lobby",
        (Some(Playing::Online), _) => "In GTA Online, private lobby",
    };
    ui.push_id(&f.key, |ui| {
        setting(ui, &f.name, Control::Action, Some(status), |ui| {
            ui.horizontal(|ui| {
                let sure = page.removing.as_deref() == Some(f.key.as_str());
                let remove = action_button(ui, if sure { "Sure?" } else { "🗙" }).on_hover_text("Remove");
                if remove.clicked() {
                    if sure {
                        friends::remove(&f.key);
                        page.removing = None;
                    } else {
                        page.removing = Some(f.key.clone());
                    }
                }
                // Joining: from the start page into its lobby; playing GTA
                // Online, into its session.
                if let Some(lobby) = &f.lobby
                    && (!game.started || game.online)
                    && action_button(ui, "Join").clicked()
                {
                    actions.push(Action::JoinFriend { key: f.key.clone(), lobby: lobby.clone() });
                }
                // Playing GTA Online: an invite to this player's session.
                if game.online && f.playing.is_some() && action_button(ui, "Invite").clicked() {
                    friends::invite(&f.key);
                    page.message = Some((format!("{} invited.", f.name), false));
                }
            });
        });
    });
}
