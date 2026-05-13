use std::time::Duration;

use bevy::app::MainSchedulePlugin;
use bevy::asset::RenderAssetUsages;
use bevy::camera::{
    ImageRenderTarget, ManualTextureViewHandle, NormalizedRenderTarget, RenderTarget,
};
use bevy::ecs::entity::{EntityHashMap, EntityHashSet};
use bevy::ecs::schedule::ScheduleLabel;
use bevy::platform::collections::HashMap;
use bevy::platform::sync::Arc;
use bevy::platform::sync::Mutex;
use bevy::platform::sync::atomic::AtomicBool;
use bevy::platform::sync::atomic::Ordering;
use bevy::prelude::*;
use bevy::render::camera::{ExtractedCamera, extract_cameras};
use bevy::render::render_asset::{RenderAsset, RenderAssets};
use bevy::render::render_resource::{
    Buffer, CommandEncoderDescriptor, Extent3d, PollType, TexelCopyBufferLayout, TextureDimension,
    TextureFormat, TextureUsages,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue};
use bevy::render::texture::{GpuImage, ManualTextureView};
use bevy::render::view::screenshot::Screenshot;
use bevy::render::{Extract, Render, RenderApp, RenderPlugin, RenderSystems};
use bevy::window::{PrimaryWindow, WindowEvent};
use ipc_channel::ipc::{self, IpcReceiver, IpcSender};
use serde::{Deserialize, Serialize};

use bevy::{
    app::AppExit,
    render::render_resource::{BufferDescriptor, BufferUsages, MapMode, TexelCopyBufferInfo},
};

use crate::{EDITOR_SERVER_NAME_VAR, EditorMsg, ExternalTexture, GameMsg};

#[derive(Default)]
pub struct EditorIntegrationPlugin;

impl Plugin for EditorIntegrationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EditorProcess>()
            .init_resource::<RenderTargets>()
            .add_systems(First, targets_first)
            .add_systems(Last, (populate_window_retargets, targets_last).chain())
            .set_runner(runner);
    }

    fn finish(&self, app: &mut App) {
        // let sender = app.world().resource::<EditorProcess>().to_editor.clone();
        app.sub_app_mut(RenderApp);
        // .insert_resource(RenderWorldSender(sender));
    }
}

#[derive(Resource)]
pub struct EditorProcess {
    to_editor: IpcSender<GameMsg>,
    // from_editor: Mutex<IpcReceiver<EditorMsg>>,
    msg_queue: Arc<Mutex<Vec<EditorMsg>>>,
}

impl Default for EditorProcess {
    fn default() -> Self {
        let (game_sender, reciever) = ipc::channel().unwrap();
        let server_name = std::env::var(EDITOR_SERVER_NAME_VAR).unwrap();
        let sender = IpcSender::connect(server_name).unwrap();
        sender.send(GameMsg::Sender(game_sender)).unwrap();
        let msg_queue = Arc::new(Mutex::new(Vec::new()));
        std::thread::spawn({
            let msg_queue = msg_queue.clone();
            move || {
                loop {
                    let msg = match reciever.recv() {
                        Ok(msg) => msg,
                        Err(err) => {
                            error!("{err}");
                            let mut queue = msg_queue.lock().unwrap();
                            queue.push(EditorMsg::Exit);
                            break;
                        }
                    };
                    let mut queue = msg_queue.lock().unwrap();
                    queue.push(msg);
                }
            }
        });
        Self {
            to_editor: sender,
            msg_queue, // from_editor: Mutex::new(reciever),
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

// TODO: all inner events should be passed and forwarded automatically
fn process_window_event(
    In(mut event): In<WindowEvent>,
    mut commands: Commands,
    window: Single<(Entity, &mut Window), With<PrimaryWindow>>,
) {
    let (window_e, mut window) = window.into_inner();

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
        WindowEvent::WindowResized(window_resized) => {
            window_resized.window = window_e;
            window
                .resolution
                .set_physical_resolution(window_resized.width as u32, window_resized.height as u32);
            commands.write_message(window_resized.clone());
        }
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
        WindowEvent::KeyboardInput(keyboard_input) => {
            keyboard_input.window = window_e;
            commands.write_message(keyboard_input.clone());
        }
        WindowEvent::KeyboardFocusLost(..) => (),
    };

    commands.write_message(event);
}

#[derive(Resource, Default)]
pub struct DisabledSystems(HashMap<String, Arc<AtomicBool>>);

fn runner(mut app: App) -> AppExit {
    app.finish();
    app.cleanup();

    // let mut editor_app = App::new();
    // editor_app.add_plugins(
    //     DefaultPlugins
    //         .build()
    //         .disable::<bevy::winit::WinitPlugin>()
    //         .disable::<bevy::log::LogPlugin>(),
    // );
    // editor_app.finish();
    // editor_app.cleanup();
    // let img =
    //     editor_app
    //         .world_mut()
    //         .resource_mut::<Assets<Image>>()
    //         .add(Image::new_target_texture(
    //             500,
    //             500,
    //             TextureFormat::Rgba8UnormSrgb,
    //             None,
    //         ));
    // editor_app
    //     .world_mut()
    //     .spawn(((Camera2d, RenderTarget::Image(img.clone().into())),));
    // editor_app.world_mut().spawn(((Sprite {
    //     custom_size: vec2(50.0, 50.0).into(),
    //     color: Color::WHITE,
    //     ..Default::default()
    // }),));

    // editor_app.add_systems(Update, {
    //     let img = img.clone();
    //     move |mut commands: Commands, time: Res<Time>, mut timer: Local<Option<Timer>>| {
    //         let timer = timer.get_or_insert_with(|| Timer::from_seconds(2.0, TimerMode::Repeating));
    //         timer.tick(time.delta());
    //         if timer.just_finished() {
    //             commands
    //                 .spawn(Screenshot::image(img.clone()))
    //                 .observe(bevy::render::view::screenshot::save_to_disk("test.png"));
    //         }
    //     }
    // });
    // let mut editor_app = core::mem::take(editor_app.sub_apps_mut());

    // let mut disabled_systems = HashMap::new();
    let mut disabled_systems = DisabledSystems::default();
    let mut schedules = app.world_mut().resource_mut::<Schedules>();
    for (l, s) in schedules.iter_mut() {
        dbg!(l);
        let systems = &mut s.graph_mut().systems;
        let labels: Vec<_> = systems.iter().map(|(label, ..)| label).collect();
        for label in labels {
            let system_name = systems.get(label).unwrap().name();
            let conditions = systems.get_conditions_mut(label).unwrap();
            let should_run = Arc::new(AtomicBool::new(true));
            disabled_systems
                .0
                .insert(system_name.as_string(), should_run.clone());
            conditions.push(bevy::ecs::schedule::ConditionWithAccess::new(Box::new(
                IntoSystem::into_system(move || should_run.load(Ordering::Relaxed)),
            )));
        }
    }

    let mut paused = false;
    let mut exit = false;
    app.world_mut()
        .resource_mut::<EditorProcess>()
        .to_editor
        .send(GameMsg::ProcessInfo {
            systems: disabled_systems.0.keys().cloned().collect(),
        });
    loop {
        app.world_mut()
            .resource_scope(|world, editor: Mut<EditorProcess>| {
                let msgs_from_game = core::mem::take(&mut *editor.msg_queue.lock().unwrap());
                for msg in msgs_from_game {
                    match msg {
                        EditorMsg::NextFrame => (),
                        EditorMsg::WindowEvent(window_event) => world
                            .run_system_cached_with(process_window_event, window_event)
                            .unwrap(),
                        EditorMsg::Pause => paused = true,
                        EditorMsg::Continue => {
                            paused = false;
                        }
                        EditorMsg::ModifySystem { name, state } => {
                            if let Some(inner) = disabled_systems.0.get(&name) {
                                inner.store(state, Ordering::SeqCst)
                            }
                        }
                        EditorMsg::Exit => exit = true,
                    }
                }
            });
        // editor_app.update();
        if !paused {
            app.update();
        }
        if exit || app.should_exit().is_some() {
            break;
        }
        // bevy::platform::thread::sleep(std::time::Duration::from_secs_f64(1.0 / 60.0));
    }

    AppExit::Success
}

struct WindowsTargets {
    render_target: RenderTarget,
    external_texture: ExternalTexture,
    last_size: UVec2,
}

#[derive(Resource, Default)]
pub struct RenderTargets {
    windows: EntityHashMap<WindowsTargets>,
    cameras: EntityHashMap<RenderTarget>,
}

fn populate_window_retargets(
    mut manual_texture_views: ResMut<ManualTextureViews>,
    mut targets: ResMut<RenderTargets>,
    device: Res<RenderDevice>,
    windows: Query<(Entity, &Window), Changed<Window>>,
    sender: Res<EditorProcess>,
) {
    for (window_e, window) in windows {
        if targets
            .windows
            .get(&window_e)
            .is_some_and(|target| target.last_size == window.physical_size())
        {
            continue;
        };
        let external_texture = ExternalTexture::new(
            device.wgpu_device(),
            &wgpu::wgt::TextureDescriptor {
                label: None,
                size: Extent3d {
                    width: window.physical_width(),
                    height: window.physical_height(),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Rgba8UnormSrgb,
                usage: TextureUsages::RENDER_ATTACHMENT
                    | TextureUsages::COPY_SRC
                    | TextureUsages::TEXTURE_BINDING,
                view_formats: &[TextureFormat::Rgba8UnormSrgb],
            },
            Some(window_e.entity().to_bits()),
        )
        .unwrap();
        let view = external_texture
            .texture()
            .create_view(&wgpu::wgt::TextureViewDescriptor {
                ..Default::default()
            });
        let texture_view =
            ManualTextureView::with_default_format(view.into(), window.physical_size());
        let texture_info = external_texture.info().clone();
        if let Some(target) = targets.windows.get_mut(&window_e) {
            let RenderTarget::TextureView(render_target) = &target.render_target else {
                panic!("Not texture view?");
            };
            manual_texture_views.insert(render_target.clone(), texture_view);
            target.external_texture = external_texture;
            target.last_size = window.physical_size();
        } else {
            let id = loop {
                let id = ManualTextureViewHandle(rand::random::<u32>());
                if !manual_texture_views.contains_key(&id) {
                    break id;
                }
            };
            manual_texture_views.insert(id.clone(), texture_view);
            let target = WindowsTargets {
                render_target: RenderTarget::TextureView(id),
                external_texture,
                last_size: window.physical_size(),
            };
            targets.windows.insert(window_e, target);
        }
        sender.to_editor.send(GameMsg::Image(texture_info)).unwrap();
    }
}

fn targets_last(
    mut cameras: Query<(Entity, &mut RenderTarget), With<Camera>>,
    mut targets: ResMut<RenderTargets>,
    primary_window: Query<Entity, With<PrimaryWindow>>,
) {
    let primary_window = primary_window.single().ok();
    for (cam_e, mut render_target) in cameras.iter_mut() {
        if let RenderTarget::Window(window_ref) = *render_target
            && let Some(window_e) = window_ref.normalize(primary_window)
        {
            let targets = targets.as_mut();
            if !targets.cameras.contains_key(&cam_e) {
                if let Some(replacement_render_target) = targets.windows.get(&window_e.entity()) {
                    targets
                        .cameras
                        .insert(cam_e, replacement_render_target.render_target.clone());
                }
            }
            let replacement_render_target = targets.cameras.get_mut(&cam_e).unwrap();
            core::mem::swap(&mut *render_target, replacement_render_target);
        };
    }
}

fn targets_first(
    mut cameras: Query<&mut RenderTarget, With<Camera>>,
    mut targets: ResMut<RenderTargets>,
) {
    for (cam_e, render_target) in targets.cameras.iter_mut() {
        if let Ok(mut replacement_render_target) = cameras.get_mut(*cam_e) {
            core::mem::swap(render_target, &mut replacement_render_target);
        }
    }
}
