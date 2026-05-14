use core::mem;
use std::time::Duration;

use bevy::app::{AppLabel, MainSchedulePlugin};
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
use bevy::render::{Extract, MainWorld, Render, RenderApp, RenderPlugin, RenderSystems};
use bevy::tasks::ComputeTaskPool;
use bevy::window::{PrimaryWindow, WindowEvent};
use ipc_channel::ipc::{self, IpcReceiver, IpcSender};
use serde::{Deserialize, Serialize};

use bevy::{
    app::AppExit,
    render::render_resource::{BufferDescriptor, BufferUsages, MapMode, TexelCopyBufferInfo},
};

use editor_common::{EDITOR_SERVER_NAME_VAR, EditorMsg, ExternalTexture, GameMsg};

#[derive(Default)]
pub struct EditorIntegrationPlugin;

impl Plugin for EditorIntegrationPlugin {
    fn build(&self, app: &mut App) {
        let Ok(server_name) = std::env::var(EDITOR_SERVER_NAME_VAR) else {
            return;
        };
        info!("Launching with editor integration");

        app.init_resource::<RenderTargets>(); // TODO: move to editor subapp

        let editor_process = EditorProcess::new(&server_name);
        let mut sub_app = SubApp::new();
        sub_app
            .insert_resource(editor_process)
            .init_resource::<RenderTargets>()
            .init_resource::<RunnerState>()
            .init_resource::<SimulationWorld>()
            .init_resource::<DisabledSystems>()
            .add_systems(IntegrationStartup, extract_systems)
            .add_systems(
                PreSimulation,
                (
                    react_to_editor_messages,
                    move |mut sim_world: ResMut<SimulationWorld>| {
                        sim_world.run_system_cached(targets_first).unwrap();
                    },
                )
                    .chain(),
            )
            .add_systems(
                PostSimulation,
                (
                    |editor_process: Res<EditorProcess>, mut sim_world: ResMut<SimulationWorld>| {
                        sim_world
                            .run_system_cached_with(
                                populate_window_retargets,
                                editor_process.to_editor.clone(),
                            )
                            .unwrap();
                    },
                    |mut sim_world: ResMut<SimulationWorld>| {
                        sim_world.run_system_cached(targets_last).unwrap();
                    },
                )
                    .chain(),
            );

        app.set_runner(runner)
            .insert_sub_app(EditorIntegrationApp, sub_app);
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, AppLabel)]
pub struct EditorIntegrationApp;

#[derive(Resource)]
pub struct EditorProcess {
    to_editor: IpcSender<GameMsg>,
    msg_queue: Arc<Mutex<Vec<EditorMsg>>>,
}

#[derive(Resource, Default, Deref, DerefMut)]
pub struct SimulationWorld(pub World);

impl EditorProcess {
    fn new(server_name: &str) -> Self {
        let (game_sender, reciever) = ipc::channel().unwrap();
        let sender = IpcSender::connect(server_name.to_string()).unwrap();
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
            msg_queue,
        }
    }

    pub fn send(&self, msg: GameMsg) {
        if let Err(e) = self.to_editor.send(msg) {
            error!("Sending failed: {e}");
        }
    }

    pub fn get_messages(&self, swap_to: &mut Vec<EditorMsg>) {
        swap_to.clear();
        core::mem::swap(&mut *self.msg_queue.lock().unwrap(), swap_to);
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

fn write_window_event(
    event: WindowEvent,
    world: &mut World,
    windows: &mut QueryState<&mut Window>,
) -> Result {
    match event.clone() {
        WindowEvent::AppLifecycle(e) => {
            world.write_message(e);
        }
        WindowEvent::CursorEntered(e) => {
            world.write_message(e);
        }
        WindowEvent::CursorLeft(e) => {
            world.write_message(e);
        }
        WindowEvent::CursorMoved(e) => {
            let mut window = windows.get_mut(world, e.window)?;
            window.set_physical_cursor_position(Some(e.position.into()));
            world.write_message(e);
        }
        WindowEvent::FileDragAndDrop(e) => {
            world.write_message(e);
        }
        WindowEvent::Ime(e) => {
            world.write_message(e);
        }
        WindowEvent::RequestRedraw(e) => {
            world.write_message(e);
        }
        WindowEvent::WindowBackendScaleFactorChanged(e) => {
            world.write_message(e);
        }
        WindowEvent::WindowCloseRequested(e) => {
            world.write_message(e);
        }
        WindowEvent::WindowCreated(e) => {
            world.write_message(e);
        }
        WindowEvent::WindowDestroyed(e) => {
            world.write_message(e);
        }
        WindowEvent::WindowFocused(e) => {
            world.write_message(e);
        }
        WindowEvent::WindowMoved(e) => {
            world.write_message(e);
        }
        WindowEvent::WindowOccluded(e) => {
            world.write_message(e);
        }
        WindowEvent::WindowResized(e) => {
            let mut window = windows.get_mut(world, e.window)?;
            window
                .resolution
                .set_physical_resolution(e.width as u32, e.height as u32);
            world.write_message(e);
        }
        WindowEvent::WindowScaleFactorChanged(e) => {
            world.write_message(e);
        }
        WindowEvent::WindowThemeChanged(e) => {
            world.write_message(e);
        }
        WindowEvent::MouseButtonInput(e) => {
            world.write_message(e);
        }
        WindowEvent::MouseMotion(e) => {
            world.write_message(e);
        }
        WindowEvent::MouseWheel(e) => {
            world.write_message(e);
        }
        WindowEvent::PinchGesture(e) => {
            world.write_message(e);
        }
        WindowEvent::RotationGesture(e) => {
            world.write_message(e);
        }
        WindowEvent::DoubleTapGesture(e) => {
            world.write_message(e);
        }
        WindowEvent::PanGesture(e) => {
            world.write_message(e);
        }
        WindowEvent::TouchInput(e) => {
            world.write_message(e);
        }
        WindowEvent::KeyboardInput(e) => {
            world.write_message(e);
        }
        WindowEvent::KeyboardFocusLost(e) => {
            world.write_message(e);
        }
    }

    world.write_message(event);

    Ok(())
}

#[derive(Resource, Default)]
struct RunnerState {
    exit: bool,
    paused: bool,
}

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct PreSimulation;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct PostSimulation;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct IntegrationStartup;

#[derive(Resource, Default)]
pub struct DisabledSystems(HashMap<String, Arc<AtomicBool>>);

fn run_schedule_with_simulation_world(
    schedule: impl ScheduleLabel,
    integration_world: &mut World,
    simulation_world: &mut World,
) {
    mem::swap(
        simulation_world,
        integration_world.resource_mut::<SimulationWorld>().as_mut(),
    );
    integration_world.run_schedule(schedule);
    mem::swap(
        simulation_world,
        integration_world.resource_mut::<SimulationWorld>().as_mut(),
    );
}

fn runner(mut app: App) -> AppExit {
    app.finish();
    app.cleanup();

    let mut editor_integration_app = app.remove_sub_app(EditorIntegrationApp).unwrap();
    let mut sub_apps = mem::take(app.sub_apps_mut());
    let integration_world = editor_integration_app.world_mut();

    run_schedule_with_simulation_world(
        IntegrationStartup,
        integration_world,
        sub_apps.main.world_mut(),
    );
    loop {
        // Pre simulation world update
        run_schedule_with_simulation_world(
            PreSimulation,
            integration_world,
            sub_apps.main.world_mut(),
        );
        if let Some(state) = integration_world.get_resource::<RunnerState>() {
            if state.paused {
                continue;
            }
            if state.exit {
                break;
            }
        };

        // Simulation world update
        sub_apps.main.run_default_schedule();

        // Post simulation world update
        run_schedule_with_simulation_world(
            PostSimulation,
            integration_world,
            sub_apps.main.world_mut(),
        );
        integration_world.clear_trackers();

        // Subapps update
        for (_, sub_app) in sub_apps.sub_apps.iter_mut() {
            sub_app.extract(sub_apps.main.world_mut());
            sub_app.update();
        }
        sub_apps.main.world_mut().clear_trackers();
    }

    AppExit::Success
}

fn extract_systems(
    mut sim_world: ResMut<SimulationWorld>,
    editor: Res<EditorProcess>,
    mut disabled_systems: ResMut<DisabledSystems>,
) {
    let mut schedules = sim_world.resource_mut::<Schedules>();
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

    editor.send(GameMsg::ProcessInfo {
        systems: disabled_systems.0.keys().cloned().collect(),
    });
}

fn react_to_editor_messages(
    editor: Res<EditorProcess>,
    mut sim_world: ResMut<SimulationWorld>,
    mut windows_query_state: Local<Option<QueryState<&mut Window>>>,
    mut runner_state: ResMut<RunnerState>,
    disabled_systems: Res<DisabledSystems>,
    mut msgs_queue: Local<Vec<EditorMsg>>,
) {
    editor.get_messages(&mut msgs_queue);
    for msg in msgs_queue.drain(..) {
        match msg {
            EditorMsg::NextFrame => (),
            EditorMsg::WindowEvent(window_event) => {
                let windows =
                    windows_query_state.get_or_insert_with(|| sim_world.query::<&mut Window>());
                if let Err(e) = write_window_event(window_event, sim_world.as_mut(), windows) {
                    error!("{e}");
                }
            }
            EditorMsg::Pause => runner_state.paused = true,
            EditorMsg::Continue => runner_state.paused = false,
            EditorMsg::Exit => runner_state.exit = true,
            EditorMsg::ModifySystem { name, state } => {
                if let Some(inner) = disabled_systems.0.get(&name) {
                    inner.store(state, Ordering::SeqCst)
                }
            }
        }
    }
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
    In(sender): In<IpcSender<GameMsg>>,
    mut manual_texture_views: ResMut<ManualTextureViews>,
    mut targets: ResMut<RenderTargets>,
    device: Res<RenderDevice>,
    windows: Query<(Entity, &Window), Changed<Window>>,
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
        sender.send(GameMsg::Image(texture_info)).unwrap();
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
