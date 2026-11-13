use rouge::platform::open_url;
#[cfg(not(target_os = "macos"))]
use rouge::reactive::spawn_tokio;
use std::future::Future;
#[cfg(not(all(unix, not(target_os = "macos"))))]
use std::future::ready;
use std::path::PathBuf;
use std::pin::Pin;
#[cfg(all(unix, not(target_os = "macos")))]
use tokio::sync::oneshot;

pub type PlatformFuture<T> = Pin<Box<dyn Future<Output = T>>>;

pub trait Platform: 'static {
    fn notify(&self, notification: Notification) -> PlatformFuture<bool>;

    fn pick_paths(&self, prompt: PathPrompt) -> PlatformFuture<Option<Vec<PathBuf>>>;

    fn open(&self, target: &str);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub tag: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPrompt {
    pub title: String,
    pub directories: bool,
    pub multiple: bool,
}

pub struct SystemPlatform;

impl Platform for SystemPlatform {
    fn notify(&self, notification: Notification) -> PlatformFuture<bool> {
        system_notification(&notification)
    }

    fn pick_paths(&self, prompt: PathPrompt) -> PlatformFuture<Option<Vec<PathBuf>>> {
        Box::pin(pick_paths(prompt))
    }

    fn open(&self, target: &str) {
        if let Err(error) = open_url(target) {
            tracing::warn!(%error, target, "could not open");
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn system_notification(notification: &Notification) -> PlatformFuture<bool> {
    let (clicked, response) = oneshot::channel();
    let mut built = notify_rust::Notification::new();
    built
        .appname(crate::menus::APP_NAME)
        .summary(&notification.title)
        .body(&notification.body)
        .action(DEFAULT_ACTION, DEFAULT_ACTION);
    let spawned =
        std::thread::Builder::new().name("system-notification".to_owned()).spawn(move || match built.show() {
            Ok(handle) => handle.wait_for_action(|action| {
                let _ = clicked.send(action == DEFAULT_ACTION);
            }),
            Err(error) => tracing::warn!(%error, "could not show a system notification"),
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start the system notification thread");
    }
    Box::pin(async move { response.await.unwrap_or(false) })
}

#[cfg(target_os = "macos")]
fn system_notification(notification: &Notification) -> PlatformFuture<bool> {
    let (title, body) = (notification.title.clone(), notification.body.clone());
    // Delivery spins a run loop; on the UI thread that re-enters winit's event handler, which aborts the app.
    let spawned = std::thread::Builder::new().name("system-notification".to_owned()).spawn(move || {
        // An unbundled app has no identifier, and without one mac-notification-sys asks macOS to find an
        // application named "use_default", which shows a blocking "Where is use_default?" dialog.
        let _ = notify_rust::set_application(NOTIFYING_APP);
        if let Err(error) = notify_rust::Notification::new().summary(&title).body(&body).show() {
            tracing::warn!(%error, "could not show a system notification");
        }
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start the system notification thread");
    }
    Box::pin(ready(false))
}

#[cfg(target_os = "macos")]
const NOTIFYING_APP: &str = "com.apple.Finder";

#[cfg(not(unix))]
fn system_notification(_notification: &Notification) -> PlatformFuture<bool> {
    Box::pin(ready(false))
}

#[cfg(all(unix, not(target_os = "macos")))]
const DEFAULT_ACTION: &str = "default";

#[cfg(target_os = "macos")]
async fn pick_paths(prompt: PathPrompt) -> Option<Vec<PathBuf>> {
    choose(prompt).await
}

#[cfg(not(target_os = "macos"))]
async fn pick_paths(prompt: PathPrompt) -> Option<Vec<PathBuf>> {
    spawn_tokio(choose(prompt)).await.ok().flatten()
}

async fn choose(prompt: PathPrompt) -> Option<Vec<PathBuf>> {
    let dialog = rfd::AsyncFileDialog::new().set_title(prompt.title);
    let picked = match (prompt.directories, prompt.multiple) {
        (true, true) => dialog.pick_folders().await,
        (true, false) => dialog.pick_folder().await.map(|folder| vec![folder]),
        (false, true) => dialog.pick_files().await,
        (false, false) => dialog.pick_file().await.map(|file| vec![file]),
    }?;
    Some(picked.into_iter().map(|handle| handle.path().to_path_buf()).collect())
}
