use bevy::platform::sync::Mutex;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowEvent};
use ipc_channel::TryRecvError;
use ipc_channel::ipc::{self, IpcReceiver, IpcSender};
use serde::{Deserialize, Serialize};

use crate::{EDITOR_SERVER_NAME_VAR, EditorMsg, GameMsg};

#[derive(Default)]
pub struct EditorIntegrationPlugin;

impl Plugin for EditorIntegrationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EditorProcess>().set_runner(runner);
    }

    fn cleanup(&self, _app: &mut App) {
        // TODO: modify schedules to be stop user code from execution
    }
}

#[derive(Resource)]
pub struct EditorProcess {
    to_editor: Mutex<IpcSender<GameMsg>>,
    from_editor: Mutex<IpcReceiver<EditorMsg>>,
}

impl Default for EditorProcess {
    fn default() -> Self {
        let (game_sender, reciever) = ipc::channel().unwrap();
        let server_name = std::env::var(EDITOR_SERVER_NAME_VAR).unwrap();
        let sender = IpcSender::connect(server_name).unwrap();
        sender.send(GameMsg::Sender(game_sender)).unwrap();
        Self {
            to_editor: Mutex::new(sender),
            from_editor: Mutex::new(reciever),
        }
    }
}

#[derive(Component, Reflect, Serialize, Deserialize, Default, Clone)]
#[reflect(Component)]
pub struct EditorSync {}

#[derive(Component, Reflect, Serialize, Deserialize)]
#[reflect(Component)]
#[require(Button)]
pub struct EditorBtn {}

#[derive(Component, Reflect, Serialize, Deserialize)]
#[reflect(Component)]
pub struct SceneEntity {}

fn process_window_event(
    In(mut event): In<WindowEvent>,
    mut message_writer: MessageWriter<WindowEvent>,
    window: Single<(Entity, &mut Window), With<PrimaryWindow>>,
) {
    let (window_e, mut window) = window.into_inner();
    info!("{event:?}");

    match &mut event {
        WindowEvent::AppLifecycle(..) => (),
        WindowEvent::CursorEntered(cursor_entered) => cursor_entered.window = window_e,
        WindowEvent::CursorLeft(cursor_left) => cursor_left.window = window_e,
        WindowEvent::FileDragAndDrop(file_drag_and_drop) => {
            match file_drag_and_drop {
                FileDragAndDrop::DroppedFile { window, .. } => *window = window_e,
                FileDragAndDrop::HoveredFile { window, .. } => *window = window_e,
                FileDragAndDrop::HoveredFileCanceled { window } => *window = window_e,
            };
        }
        WindowEvent::Ime(ime) => {
            match ime {
                Ime::Preedit { window, .. } => *window = window_e,
                Ime::Commit { window, .. } => *window = window_e,
                Ime::Enabled { window } => *window = window_e,
                Ime::Disabled { window } => *window = window_e,
            };
        }
        WindowEvent::RequestRedraw(..) => (),
        WindowEvent::WindowBackendScaleFactorChanged(window_backend_scale_factor_changed) => {
            window_backend_scale_factor_changed.window = window_e
        }
        WindowEvent::WindowCloseRequested(window_close_requested) => {
            window_close_requested.window = window_e
        }
        WindowEvent::WindowCreated(window_created) => window_created.window = window_e,
        WindowEvent::WindowDestroyed(window_destroyed) => window_destroyed.window = window_e,
        WindowEvent::WindowFocused(window_focused) => window_focused.window = window_e,
        WindowEvent::WindowMoved(window_moved) => window_moved.window = window_e,
        WindowEvent::WindowOccluded(window_occluded) => window_occluded.window = window_e,
        WindowEvent::WindowResized(window_resized) => window_resized.window = window_e,
        WindowEvent::WindowScaleFactorChanged(window_scale_factor_changed) => {
            window_scale_factor_changed.window = window_e
        }
        WindowEvent::WindowThemeChanged(window_theme_changed) => {
            window_theme_changed.window = window_e
        }
        WindowEvent::CursorMoved(cursor_moved) => {
            window.set_physical_cursor_position(Some(cursor_moved.position.into()));
            cursor_moved.window = window_e;
        }
        WindowEvent::MouseMotion(..) => (),
        WindowEvent::MouseWheel(mouse_wheel) => mouse_wheel.window = window_e,
        WindowEvent::MouseButtonInput(mouse_button_input) => mouse_button_input.window = window_e,
        WindowEvent::PinchGesture(..) => (),
        WindowEvent::RotationGesture(..) => (),
        WindowEvent::DoubleTapGesture(..) => (),
        WindowEvent::PanGesture(..) => (),
        WindowEvent::TouchInput(touch_input) => touch_input.window = window_e,
        WindowEvent::KeyboardInput(keyboard_input) => keyboard_input.window = window_e,
        WindowEvent::KeyboardFocusLost(..) => (),
    };

    message_writer.write(event);
}

fn runner(mut app: App) -> AppExit {
    app.finish();
    app.cleanup();

    loop {
        app.world_mut()
            .resource_scope(|world, editor: Mut<EditorProcess>| {
                let from_editor = editor.from_editor.lock().unwrap();
                loop {
                    let msg = from_editor.recv().unwrap();
                    match msg {
                        EditorMsg::NextFrame => break,
                        EditorMsg::Number(num) => info!("{num}"),
                        EditorMsg::WindowEvent(window_event) => world
                            .run_system_cached_with(process_window_event, window_event)
                            .unwrap(),
                    }
                }
            });
        app.update();
        if app.should_exit().is_some() {
            break;
        }
        // bevy::platform::thread::sleep(Duration::from_secs_f64(1.0 / 60.0));
    }

    AppExit::Success
}
