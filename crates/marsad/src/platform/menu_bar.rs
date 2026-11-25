//! Present owner actions from AppKit while the daemon serves on its own runtime.

#![allow(unsafe_code)]

use std::{
    cell::{Cell, RefCell},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use dispatch2::DispatchQueue;
use marsa_core::settings::Language;
use marsa_l10n::{Locale, keys};
use marsa_proto::host::v1::EngineState;
use marsad::typed_api::{
    TypedApi,
    menu_bar::{MenuBarEngine, MenuBarState},
};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, rc::Retained, runtime::NSObject,
    sel,
};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSApplicationActivationPolicy, NSEvent,
    NSEventModifierFlags, NSEventType, NSImage, NSMenu, NSMenuItem, NSModalResponseCancel,
    NSRunningApplication, NSStatusBar, NSStatusItem, NSUserInterfaceLayoutDirection,
    NSVariableStatusItemLength, NSWorkspace, NSWorkspaceOpenConfiguration,
};
use objc2_core_foundation::{
    CFRunLoop, CFRunLoopMode, kCFRunLoopCommonModes, kCFRunLoopDefaultMode,
};
use objc2_foundation::{NSDictionary, NSLocale, NSPoint, NSString, NSURL};
use objc2_security::{SessionAttributeBits, SessionGetInfo, callerSecuritySession};
use tokio::sync::{mpsc, oneshot};

use super::DaemonError;

thread_local! {
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

pub(super) struct DaemonUi {
    pub(super) quit: oneshot::Receiver<()>,
    commands: mpsc::Receiver<MenuBarEngine>,
}

struct ControllerState {
    bundle: PathBuf,
    commands: mpsc::Sender<MenuBarEngine>,
    quit: RefCell<Option<oneshot::Sender<()>>>,
    state: Cell<MenuBarState>,
    opening: Arc<AtomicBool>,
}

define_class!(
    // SAFETY: NSObject permits subclassing. AppKit invokes these selectors
    // on this main-thread-only target, and each action takes one menu item.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MarsaMenuBarController"]
    #[ivars = ControllerState]
    struct Controller;

    impl Controller {
        #[unsafe(method(startEngine:))]
        fn start(&self, sender: &NSMenuItem) {
            if self.ivars().commands.try_send(MenuBarEngine::Start).is_ok() {
                sender.setEnabled(false);
            }
        }

        #[unsafe(method(stopEngine:))]
        fn stop(&self, sender: &NSMenuItem) {
            let locale = locale(self.ivars().state.get().language);
            if confirm(
                self.mtm(),
                keys::GUI_DESKTOP_STOP_THE_MACHINE.text(locale),
                keys::GUI_DESKTOP_STOP_MACHINE.text(locale),
                locale,
            ) && self.ivars().commands.try_send(MenuBarEngine::Stop).is_ok() {
                sender.setEnabled(false);
            }
        }

        #[unsafe(method(openDesktop:))]
        fn open(&self, _sender: &NSMenuItem) {
            let state = self.ivars();
            let Some(path) = state.bundle.to_str() else { return };
            if state.opening.swap(true, Ordering::AcqRel) {
                return;
            }
            let open = NSWorkspaceOpenConfiguration::configuration();
            // The daemon can share the outer bundle identifier. A process
            // running the desktop executable is the only reusable instance.
            let executable = state.bundle.join("Contents/MacOS/marsa-gui");
            let desktop_running = NSWorkspace::sharedWorkspace()
                .runningApplications()
                .iter()
                .filter_map(|application| application.executableURL()?.path())
                .any(|candidate| Path::new(&candidate.to_string()) == executable);
            open.setCreatesNewApplicationInstance(!desktop_running);
            if !desktop_running {
                open.setEnvironment(&desktop_environment());
            }
            let opening = Arc::clone(&state.opening);
            let completion = block2::RcBlock::new(move |application: *mut NSRunningApplication, _error: *mut objc2_foundation::NSError| {
                opening.store(false, Ordering::Release);
                if application.is_null() {
                    tracing::warn!("menu bar could not open the bundled desktop");
                }
            });
            NSWorkspace::sharedWorkspace().openApplicationAtURL_configuration_completionHandler(
                &NSURL::fileURLWithPath_isDirectory(&NSString::from_str(path), true),
                &open,
                Some(&completion),
            );
        }

        #[unsafe(method(quitMarsa:))]
        fn quit(&self, _sender: &NSMenuItem) {
            let locale = locale(self.ivars().state.get().language);
            if confirm(self.mtm(), keys::GUI_MENU_QUIT_CONFIRM.text(locale), keys::GUI_MENU_QUIT.text(locale), locale)
                && let Some(quit) = self.ivars().quit.borrow_mut().take()
            {
                let _ = quit.send(());
            }
        }
    }
);

struct Tray {
    bar: Retained<NSStatusBar>,
    item: Retained<NSStatusItem>,
    menu: Retained<NSMenu>,
    controller: Retained<Controller>,
    engine: Retained<NSMenuItem>,
    approvals: Retained<NSMenuItem>,
    failure: Retained<NSMenuItem>,
    start: Retained<NSMenuItem>,
    stop: Retained<NSMenuItem>,
    open: Retained<NSMenuItem>,
    quit: Retained<NSMenuItem>,
}

impl Tray {
    fn new(
        mtm: MainThreadMarker,
        bundle: PathBuf,
        commands: mpsc::Sender<MenuBarEngine>,
        quit: oneshot::Sender<()>,
    ) -> Option<Self> {
        let initial = MenuBarState {
            engine: EngineState::Unspecified,
            language: Language::System,
            pending_approvals: 0,
        };
        let allocated = Controller::alloc(mtm).set_ivars(ControllerState {
            bundle,
            commands,
            quit: RefCell::new(Some(quit)),
            state: Cell::new(initial),
            opening: Arc::new(AtomicBool::new(false)),
        });
        // SAFETY: NSObject is the superclass and its init accepts this allocated controller.
        let controller: Retained<Controller> = unsafe { objc2::msg_send![super(allocated), init] };
        let bar = NSStatusBar::systemStatusBar();
        let item = bar.statusItemWithLength(NSVariableStatusItemLength);
        let Some(button) = item.button(mtm) else {
            bar.removeStatusItem(&item);
            return None;
        };
        let name = NSString::from_str("Marsa");
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("shippingbox"),
            Some(&name),
        ) {
            image.setTemplate(true);
            button.setImage(Some(&image));
        } else {
            button.setTitle(&name);
        }
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        item.setMenu(Some(&menu));

        let row = |action| {
            let row = NSMenuItem::new(mtm);
            if let Some(selector) = action {
                // SAFETY: The controller is retained for the menu's entire
                // lifetime and implements each selector with the NSMenuItem sender signature.
                unsafe {
                    row.setTarget(Some(&controller));
                    row.setAction(Some(selector));
                }
            } else {
                row.setEnabled(false);
            }
            menu.addItem(&row);
            row
        };
        let engine = row(None);
        let approvals = row(None);
        let failure = row(None);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let start = row(Some(sel!(startEngine:)));
        let stop = row(Some(sel!(stopEngine:)));
        let open = row(Some(sel!(openDesktop:)));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let quit = row(Some(sel!(quitMarsa:)));
        let tray = Self {
            bar,
            item,
            menu,
            controller,
            engine,
            approvals,
            failure,
            start,
            stop,
            open,
            quit,
        };
        tray.update(initial, false);
        Some(tray)
    }

    fn update(&self, state: MenuBarState, failed: bool) {
        self.controller.ivars().state.set(state);
        let locale = locale(state.language);
        if let Some(button) = self.item.button(self.controller.mtm()) {
            let title = NSString::from_str(keys::GUI_WINDOW_TITLE.text(locale));
            button.setToolTip(Some(&title));
            if let Some(image) = button.image() {
                image.setAccessibilityDescription(Some(&title));
            } else {
                button.setTitle(&title);
            }
        }
        self.menu.setUserInterfaceLayoutDirection(match locale {
            Locale::EnglishUs => NSUserInterfaceLayoutDirection::LeftToRight,
            Locale::Arabic => NSUserInterfaceLayoutDirection::RightToLeft,
        });
        let title = match state.engine {
            EngineState::Unspecified => keys::GUI_MENU_ENGINE_UNAVAILABLE,
            EngineState::Stopped => keys::GUI_MENU_ENGINE_STOPPED,
            EngineState::Starting => keys::GUI_MENU_ENGINE_STARTING,
            EngineState::Booting => keys::GUI_MENU_ENGINE_BOOTING,
            EngineState::Ready => keys::GUI_MENU_ENGINE_READY,
            EngineState::Idle => keys::GUI_MENU_ENGINE_IDLE,
            EngineState::Paused => keys::GUI_MENU_ENGINE_PAUSED,
            EngineState::Stopping => keys::GUI_MENU_ENGINE_STOPPING,
            EngineState::Failed => keys::GUI_MENU_ENGINE_FAILED,
        };
        self.failure.setTitle(&NSString::from_str(
            keys::GUI_MENU_ENGINE_ACTION_FAILED.text(locale),
        ));
        self.failure.setHidden(!failed);
        self.engine
            .setTitle(&NSString::from_str(title.text(locale)));
        self.approvals.setTitle(&NSString::from_str(&format!(
            "{} · {}",
            keys::GUI_DESKTOP_PENDING_AGENT_APPROVALS.text(locale),
            state.pending_approvals
        )));
        for (row, label) in [
            (&self.start, keys::GUI_DESKTOP_START_MACHINE),
            (&self.stop, keys::GUI_DESKTOP_STOP_MACHINE),
            (&self.open, keys::GUI_MENU_OPEN),
            (&self.quit, keys::GUI_MENU_QUIT),
        ] {
            row.setTitle(&NSString::from_str(label.text(locale)));
        }
        self.start.setEnabled(matches!(
            state.engine,
            EngineState::Stopped | EngineState::Failed | EngineState::Paused
        ));
        self.stop.setEnabled(matches!(
            state.engine,
            EngineState::Starting
                | EngineState::Booting
                | EngineState::Ready
                | EngineState::Idle
                | EngineState::Paused
        ));
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.item.setMenu(None);
        self.bar.removeStatusItem(&self.item);
    }
}

impl DaemonUi {
    pub(super) fn attach(&mut self, api: &TypedApi) {
        let mut model = api.watch_menu_bar();
        let api = api.clone();
        let (_, placeholder) = mpsc::channel(1);
        let mut commands = std::mem::replace(&mut self.commands, placeholder);
        tokio::spawn(async move {
            let mut actions = tokio::task::JoinSet::new();
            let mut generation = 0_u64;
            update_on_main(*model.borrow_and_update(), false);
            loop {
                tokio::select! {
                    Ok(()) = model.changed() => update_on_main(*model.borrow_and_update(), false),
                    command = commands.recv() => {
                        let Some(command) = command else { break };
                        let api = api.clone();
                        // The supervisor owns serialization and can cancel a
                        // boot even while the Start caller awaits its reply.
                        generation = generation.wrapping_add(1);
                        actions.spawn(async move { (generation, api.menu_bar_engine(command).await) });
                    }
                    Some(Ok((completed, result))) = actions.join_next(), if !actions.is_empty() => {
                        if completed == generation {
                            update_on_main(*model.borrow_and_update(), result.is_err());
                        }
                    }
                }
            }
        });
    }
}

pub(super) fn run(
    bundle: PathBuf,
    daemon: impl FnOnce(Option<DaemonUi>) -> Result<(), DaemonError> + Send + 'static,
) -> Result<(), DaemonError> {
    let Some(mtm) = MainThreadMarker::new().filter(|_| graphical_session()) else {
        return daemon(None);
    };
    let application = NSApplication::sharedApplication(mtm);
    if !application.setActivationPolicy(NSApplicationActivationPolicy::Accessory) {
        return daemon(None);
    }
    let (start, commands) = mpsc::channel(2);
    let (quit, stop) = oneshot::channel();
    let Some(tray) = Tray::new(mtm, bundle, start, quit) else {
        return daemon(None);
    };
    TRAY.with(|slot| slot.replace(Some(tray)));
    let worker = std::thread::Builder::new()
        .name("marsa-daemon".into())
        .spawn(move || {
            let _finished = StopAppKit;
            daemon(Some(DaemonUi {
                quit: stop,
                commands,
            }))
        })?;
    application.run();
    TRAY.with(|slot| slot.replace(None));
    worker
        .join()
        .map_err(|_| io::Error::other("daemon worker panicked"))?
}

struct StopAppKit;

impl Drop for StopAppKit {
    fn drop(&mut self) {
        // SAFETY: These immutable system constants describe the run loop modes.
        if let Some(common) = unsafe { kCFRunLoopCommonModes } {
            schedule_appkit(common, stop_application);
        }
    }
}

fn schedule_appkit(mode: &CFRunLoopMode, work: impl Fn() + Send + Sync + 'static) {
    let Some(run_loop) = CFRunLoop::main() else {
        return;
    };
    let callback = block2::RcBlock::new(work);
    // SAFETY: mode points to a CFString. Core Foundation copies the block and
    // invokes it in the main run loop, including nested modal modes when requested.
    unsafe { run_loop.perform_block(Some(mode), Some(&callback)) };
    run_loop.wake_up();
}

fn stop_application() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let application = NSApplication::sharedApplication(mtm);
    if application.modalWindow().is_some() {
        // A stop during a confirmation otherwise stops only that modal
        // loop. Cancel the confirmation and stop the app after it unwinds.
        application.stopModalWithCode(NSModalResponseCancel);
        // SAFETY: The default mode constant is immutable. Waiting for that
        // mode lets the nested AppKit confirmation unwind before stopping the app.
        if let Some(default) = unsafe { kCFRunLoopDefaultMode } {
            schedule_appkit(default, stop_application);
        }
    } else {
        application.stop(None);
    }
    // AppKit finishes its loop after processing an event. Wake its
    // event wait when the daemon completed while the menu bar was idle.
    if let Some(event) = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::ApplicationDefined, NSPoint::ZERO, NSEventModifierFlags::empty(), 0.0, 0, None, 0, 0, 0,
    ) {
        application.postEvent_atStart(&event, true);
    }
}

fn graphical_session() -> bool {
    let mut attributes = SessionAttributeBits::empty();
    // SAFETY: The caller's session permits a null ID output. Attributes
    // points to initialized, writable storage for the duration of the call.
    let result =
        unsafe { SessionGetInfo(callerSecuritySession, std::ptr::null_mut(), &mut attributes) };
    result == 0 && attributes.contains(SessionAttributeBits::sessionHasGraphicAccess)
}

fn desktop_environment() -> Retained<NSDictionary<NSString, NSString>> {
    let scoped: Vec<_> = ["HOME", "DOCKER_CONFIG"]
        .into_iter()
        .filter_map(|key| {
            let value = std::env::var(key).ok()?;
            Some((NSString::from_str(key), NSString::from_str(&value)))
        })
        .collect();
    let (keys, values): (Vec<_>, Vec<_>) =
        scoped.iter().map(|(key, value)| (&**key, &**value)).unzip();
    NSDictionary::from_slices(&keys, &values)
}

fn update_on_main(state: MenuBarState, failed: bool) {
    DispatchQueue::main().exec_async(move || {
        TRAY.with(|slot| {
            if let Some(tray) = slot.borrow().as_ref() {
                tray.update(state, failed);
            }
        });
    });
}

fn locale(language: Language) -> Locale {
    match language {
        Language::Arabic => Locale::Arabic,
        Language::English => Locale::EnglishUs,
        Language::System => {
            let arabic = NSLocale::preferredLanguages()
                .firstObject()
                .is_some_and(|language| {
                    language.to_string().split(['-', '_']).next() == Some("ar")
                });
            if arabic {
                Locale::Arabic
            } else {
                Locale::EnglishUs
            }
        }
    }
}

fn confirm(mtm: MainThreadMarker, message: &str, action: &str, locale: Locale) -> bool {
    NSApplication::sharedApplication(mtm).activate();
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(message));
    alert.addButtonWithTitle(&NSString::from_str(action));
    alert.addButtonWithTitle(&NSString::from_str(keys::GUI_DESKTOP_CANCEL.text(locale)));
    alert.runModal() == NSAlertFirstButtonReturn
}
