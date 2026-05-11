use std::time::Duration;

use bevy::asset::RenderAssetUsages;
use bevy::camera::{
    ImageRenderTarget, ManualTextureViewHandle, NormalizedRenderTarget, RenderTarget,
};
use bevy::ecs::entity::{EntityHashMap, EntityHashSet};
use bevy::platform::sync::Arc;
use bevy::platform::sync::Mutex;
use bevy::prelude::*;
use bevy::render::camera::{ExtractedCamera, extract_cameras};
use bevy::render::render_asset::{RenderAsset, RenderAssets};
use bevy::render::render_resource::{
    Buffer, CommandEncoderDescriptor, Extent3d, PollType, TexelCopyBufferLayout, TextureDimension,
    TextureFormat, TextureUsages,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue};
use bevy::render::texture::{GpuImage, ManualTextureView};
use bevy::render::{Extract, Render, RenderApp, RenderPlugin, RenderSystems};
use bevy::window::{PrimaryWindow, WindowEvent};
use ipc_channel::ipc::{self, IpcReceiver, IpcSender};
use serde::{Deserialize, Serialize};

use bevy::{
    app::AppExit,
    render::render_resource::{BufferDescriptor, BufferUsages, MapMode, TexelCopyBufferInfo},
};

use crate::{EDITOR_SERVER_NAME_VAR, EditorMsg, GameMsg};

#[derive(Default)]
pub struct EditorIntegrationPlugin;

impl Plugin for EditorIntegrationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EditorProcess>()
            .init_resource::<RenderTargets>()
            .add_systems(First, targets_first)
            .add_systems(Last, targets_last)
            .set_runner(runner);
    }

    fn finish(&self, app: &mut App) {
        // let sender = app.world().resource::<EditorProcess>().to_editor.clone();
        app.sub_app_mut(RenderApp);
        // .insert_resource(RenderWorldSender(sender));
    }

    fn cleanup(&self, _app: &mut App) {
        // TODO: modify schedules to be stop user code from execution
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
                    let msg = reciever.recv().unwrap();
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

    let mut paused = false;
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
                        EditorMsg::Continue => paused = false,
                    }
                }
            });
        if !paused {
            app.update();
        }
        if app.should_exit().is_some() {
            break;
        }
        // bevy::platform::thread::sleep(std::time::Duration::from_secs_f64(1.0 / 60.0));
    }

    AppExit::Success
}

#[derive(Resource, Default)]
pub struct RenderTargets(EntityHashMap<(RenderTarget, EntityHashSet)>);

fn targets_last(
    mut cameras: Query<(Entity, &mut RenderTarget), With<Camera>>,
    mut targets: ResMut<RenderTargets>,
    mut manual_texture_views: ResMut<ManualTextureViews>,
    device: Res<RenderDevice>,
    primary_window: Query<Entity, With<PrimaryWindow>>,
    sender: Res<EditorProcess>,
) {
    let primary_window = primary_window.single().ok();
    for (cam_e, mut render_target) in cameras.iter_mut() {
        if let RenderTarget::Window(window_ref) = *render_target
            && let Some(window_e) = window_ref.normalize(primary_window)
        {
            let (old_render_target, cams) =
                targets.0.entry(window_e.entity()).or_insert_with(|| {
                    let id = loop {
                        let id = ManualTextureViewHandle(rand::random::<u32>());
                        if !manual_texture_views.contains_key(&id) {
                            break id;
                        }
                    };
                    let texture = unsafe {
                        let texture = crate::external_texture::create_exportable_texture(
                            device.wgpu_device(),
                            &wgpu::wgt::TextureDescriptor {
                                label: None,
                                size: Extent3d {
                                    width: 1280,
                                    height: 700,
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
                        )
                        .unwrap();
                        let (fd, meta) =
                            crate::external_texture::export_texture(device.wgpu_device(), &texture)
                                .unwrap();
                        core::mem::forget(fd);
                        sender.to_editor.send(GameMsg::Image(meta)).unwrap();
                        texture
                    };
                    let view = texture.create_view(&wgpu::wgt::TextureViewDescriptor {
                        ..Default::default()
                    });
                    core::mem::forget(texture);

                    let texture_view =
                        ManualTextureView::with_default_format(view.into(), uvec2(1280, 700));
                    manual_texture_views.insert(id.clone(), texture_view);
                    (RenderTarget::TextureView(id), Default::default())
                });
            cams.insert(cam_e);
            core::mem::swap(&mut *render_target, old_render_target);
        };
    }
}

fn targets_first(
    mut cameras: Query<&mut RenderTarget, With<Camera>>,
    mut targets: ResMut<RenderTargets>,
) {
    for (e, (old_render_target, cams)) in targets.0.iter_mut() {
        for cam in cams.iter() {
            if let Ok(mut render_target) = cameras.get_mut(*cam) {
                core::mem::swap(&mut *render_target, old_render_target);
            }
        }
    }
}

#[derive(Resource, Deref)]
struct RenderWorldSender(IpcSender<GameMsg>);
