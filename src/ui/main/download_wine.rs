// ============================================================
// FIX REQUIRED IN THE PARENT COMPONENT (not in this file):
//
// Wherever you launch the DownloadWine component, you MUST
// store the returned Controller in your parent model struct,
// otherwise it is immediately dropped and kills the runtime.
//
//   ❌ Wrong – controller dropped instantly:
//      DownloadWine::builder().launch(()).detach();
//
//   ✅ Option A – store in parent model:
//      pub struct App {
//          download_wine_controller: Option<Controller<DownloadWine>>,
//      }
//      // then when launching:
//      self.download_wine_controller = Some(
//          DownloadWine::builder()
//              .launch(())
//              .forward(sender.input_sender(), |msg| msg)
//      );
//
//   ✅ Option B – detach the runtime so it outlives the controller:
//      DownloadWine::builder().launch(()).detach_runtime();
// ============================================================

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use relm4::{
    prelude::*,
    Sender
};

use gtk::glib::clone;

use anime_launcher_sdk::components::wine;

use crate::*;
use crate::ui::components::*;

use super::{App, AppMsg};

/// Attempt to send a message via ComponentSender, logging a warning instead
/// of panicking if the component runtime has already been shut down.
///
/// relm4's ComponentSender::input() panics if the receiver is gone.
/// This wrapper catches that case gracefully.
macro_rules! try_send {
    ($sender:expr, $msg:expr) => {
        // ComponentSender::input panics on a closed channel; we catch it here
        // so a dropped controller doesn't crash the whole process.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            $sender.input($msg);
        })) {
            Ok(_) => {}
            Err(_) => {
                tracing::warn!(
                    "Component sender was closed (controller dropped) \
                     before the download thread finished — message lost."
                );
            }
        }
    };
}

pub fn download_wine(sender: ComponentSender<App>, progress_bar_input: Sender<ProgressBarMsg>) {
    let mut config = match Config::get() {
        Ok(cfg) => cfg,
        Err(err) => {
            sender.input(AppMsg::Toast {
                title: tr!("config-load-failed"),
                description: Some(err.to_string()),
            });
            return;
        }
    };

    match wine::get_downloaded(&CONFIG.components.path, &config.game.wine.builds) {
        Ok(downloaded) => {
            // ── A downloaded version already exists ──────────────────────────
            if !downloaded.is_empty() {
                config.game.wine.selected =
                    Some(downloaded[0].versions[0].name.clone());

                Config::update(config);

                sender.input(AppMsg::UpdateLauncherState {
                    perform_on_download_needed: false,
                    show_status_page: true,
                });

                return;
            }

            // ── No version available — download the latest ───────────────────
            let latest = match wine::Version::latest(&CONFIG.components.path) {
                Ok(v) => v,
                Err(err) => {
                    sender.input(AppMsg::Toast {
                        title: tr!("wine-latest-failed"),
                        description: Some(err.to_string()),
                    });
                    return;
                }
            };

            // Prefer the user's selected version; fall back to latest.
            let wine = match &config.game.wine.selected {
                Some(version) => {
                    match wine::Version::find_in(&config.components.path, version) {
                        Ok(Some(v)) => v,
                        _ => latest,
                    }
                }
                None => latest,
            };

            let mut installer = match Installer::new(wine.uri) {
                Ok(i) => i,
                Err(err) => {
                    sender.input(AppMsg::Toast {
                        title: tr!("wine-install-failed"),
                        description: Some(err.to_string()),
                    });
                    return;
                }
            };

            if let Some(temp_folder) = &config.launcher.temp {
                installer.temp_folder = temp_folder.to_path_buf();
            }

            sender.input(AppMsg::SetDownloading(true));

            std::thread::spawn(clone!(
                #[strong]
                sender,

                move || {
                    installer.install(
                        &config.game.wine.builds,
                        clone!(
                            #[strong]
                            sender,

                            move |state| {
                                match &state {
                                    InstallerUpdate::DownloadingError(err) => {
                                        tracing::error!("Downloading failed: {err}");

                                        try_send!(sender, AppMsg::Toast {
                                            title: tr!("downloading-failed"),
                                            description: Some(err.to_string()),
                                        });
                                    }

                                    InstallerUpdate::UnpackingError(err) => {
                                        tracing::error!("Unpacking failed: {err}");

                                        try_send!(sender, AppMsg::Toast {
                                            title: tr!("unpacking-failed"),
                                            description: Some(err.clone()),
                                        });
                                    }

                                    _ => {}
                                }

                                // progress_bar_input.send already returns Result;
                                // ignore the error if the bar was unmounted.
                                let _ = progress_bar_input.send(
                                    ProgressBarMsg::UpdateFromState(
                                        DiffUpdate::InstallerUpdate(state),
                                    ),
                                );
                            }
                        ),
                    );

                    config.game.wine.selected = Some(wine.name.clone());
                    Config::update(config);

                    // Use try_send! so a dropped controller doesn't panic here.
                    try_send!(sender, AppMsg::SetDownloading(false));
                    try_send!(sender, AppMsg::UpdateLauncherState {
                        perform_on_download_needed: false,
                        show_status_page: true,
                    });
                }
            ));
        }

        Err(err) => {
            sender.input(AppMsg::Toast {
                title: tr!("downloaded-wine-list-failed"),
                description: Some(err.to_string()),
            });
        }
    }
}
