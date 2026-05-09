use std::time::Duration;

use bevy::prelude::*;
use bevy::remote::RemotePlugin;
use bevy::remote::http::RemoteHttpPlugin;
use bevy::ui_widgets::Activate;
use bevy::window::{PrimaryWindow, WindowEvent};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{EditorMsg, EditorProcess};

#[derive(Default)]
pub struct EditorIntegrationPlugin;

impl Plugin for EditorIntegrationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EditorProcess>()
            .add_plugins((
                RemotePlugin::default()
                    .with_method_main(
                        BRP_SEND_WINDOW_MESSAGE_METHOD,
                        process_window_message_request,
                    )
                    .with_method_main(
                        BRP_TRIGGER_ACTIVATE_EVENT_METHOD,
                        process_trigger_activate_event_request,
                    ),
                RemoteHttpPlugin::default(),
                bevy::feathers::FeathersPlugins,
            ))
            .set_runner(runner);
    }

    fn cleanup(&self, _app: &mut App) {
        // TODO: modify schedules to be stop user code from execution
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

pub const BRP_SEND_WINDOW_MESSAGE_METHOD: &str = "world.send_window_message";

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct BrpSendWindowMessageParams {
    pub messages: Vec<WindowEvent>,
}

fn process_window_message_request(
    In(params): In<Option<Value>>,
    mut message_writer: MessageWriter<WindowEvent>,
    window: Single<(Entity, &mut Window), With<PrimaryWindow>>,
) -> bevy::remote::BrpResult {
    let BrpSendWindowMessageParams { messages } =
        bevy::remote::builtin_methods::parse_some(params)?;
    let (window_e, mut window) = window.into_inner();
    // window.focused = true;

    message_writer.write_batch(messages.into_iter().map(|mut f| {
        match &mut f {
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
            WindowEvent::MouseButtonInput(mouse_button_input) => {
                mouse_button_input.window = window_e
            }
            WindowEvent::PinchGesture(..) => (),
            WindowEvent::RotationGesture(..) => (),
            WindowEvent::DoubleTapGesture(..) => (),
            WindowEvent::PanGesture(..) => (),
            WindowEvent::TouchInput(touch_input) => touch_input.window = window_e,
            WindowEvent::KeyboardInput(keyboard_input) => keyboard_input.window = window_e,
            WindowEvent::KeyboardFocusLost(..) => (),
        }
        f
    }));

    Ok(().into())
}

pub const BRP_TRIGGER_ACTIVATE_EVENT_METHOD: &str = "world.trigger_activate_event";

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct BrpTriggerActivateEvent {
    pub entity: Entity,
}

fn process_trigger_activate_event_request(
    In(params): In<Option<Value>>,
    world: &mut World,
) -> bevy::remote::BrpResult {
    let BrpTriggerActivateEvent { entity } = bevy::remote::builtin_methods::parse_some(params)?;
    info!("recv {:?}", entity);

    world.trigger(Activate { entity });
    Ok(().into())
}

fn runner(mut app: App) -> AppExit {
    app.finish();
    app.cleanup();

    loop {
        app.update();
        let mut proc = app.world_mut().resource_mut::<EditorProcess>();
        loop {
            let msg = proc.ipc.recv();
            if let Some(msg) = msg {
                info!("{msg:?}");
                if let EditorMsg::NextFrame = msg {
                    break;
                }
            }
        }
        // while let Some(msg) = proc.ipc.recv() {
        //     info!("{:?}", msg);
        // }
        if app.should_exit().is_some() {
            break;
        }
        // bevy::platform::thread::sleep(Duration::from_secs_f64(1.0 / 60.0));
    }

    AppExit::Success
}
