use std::time::Duration;

use bevy::asset::RenderAssetUsages;
use bevy::camera::{ImageRenderTarget, NormalizedRenderTarget, RenderTarget};
use bevy::ecs::entity::EntityHashMap;
use bevy::platform::sync::Arc;
use bevy::platform::sync::Mutex;
use bevy::prelude::*;
use bevy::render::camera::{ExtractedCamera, extract_cameras};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    Buffer, CommandEncoderDescriptor, Extent3d, PollType, TexelCopyBufferLayout, TextureDimension,
    TextureFormat, TextureUsages,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue};
use bevy::render::texture::GpuImage;
use bevy::render::{Extract, Render, RenderApp, RenderSystems};
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
            .init_resource::<WindowsToImages>()
            .init_resource::<RenderTargets>()
            .add_systems(PostUpdate, prepare_window_target_textures)
            .add_systems(First, targets_first)
            .add_systems(Last, targets_last)
            .set_runner(runner);
    }

    fn finish(&self, app: &mut App) {
        let sender = app.world().resource::<EditorProcess>().to_editor.clone();
        app.sub_app_mut(RenderApp)
            .init_resource::<ImageCopiers>()
            .init_resource::<WindowsToImages>()
            .insert_resource(RenderWorldSender(sender))
            .add_systems(
                ExtractSchedule,
                replace_window_targets_in_extracted_cameras.after(extract_cameras),
            )
            .add_systems(
                Render,
                receive_image_from_buffer.after(RenderSystems::Render),
            )
            .add_systems(RenderGraph, image_copy_driver);
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
    // info!("{event:?}");

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

#[derive(Component)]
pub struct TestCam;

#[derive(Resource, Default)]
pub struct WindowsToImages(EntityHashMap<Handle<Image>>);

#[derive(Resource, Default)]
pub struct RenderTargets(EntityHashMap<RenderTarget>);

fn targets_last(
    mut cameras: Query<(Entity, &mut RenderTarget)>,
    windows_to_images: Res<WindowsToImages>,
    mut targets: ResMut<RenderTargets>,
    primary_window: Query<Entity, With<PrimaryWindow>>,
) {
    let primary_window = primary_window.single().unwrap();
    for (e, mut render_target) in cameras.iter_mut() {
        let RenderTarget::Window(window_ref) = *render_target else {
            continue;
        };
        let Some(image) = windows_to_images
            .0
            .get(&window_ref.normalize(Some(primary_window)).unwrap().entity())
            .cloned()
        else {
            continue;
        };
        targets.0.insert(
            e,
            core::mem::replace(&mut *render_target, RenderTarget::Image(image.into())),
        );
    }
}

fn targets_first(mut cameras: Query<&mut RenderTarget>, mut targets: ResMut<RenderTargets>) {
    for (e, old_render_target) in targets.0.drain() {
        let Ok(mut render_target) = cameras.get_mut(e) else {
            continue;
        };
        _ = core::mem::replace(&mut *render_target, old_render_target);
    }
}

pub fn replace_render_targets(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut windows_to_images: ResMut<WindowsToImages>,
    windows: Query<(Entity, &Window), Added<Window>>,
    cameras: Query<Entity, (Added<Camera>, With<TestCam>)>,
) {
    for (window_e, window) in windows.iter() {
        let image = images.add(new_render_target(
            window.physical_height(),
            window.physical_width(),
        ));
        for cam in cameras.iter() {
            commands
                .entity(cam)
                .insert(Into::<RenderTarget>::into(image.clone()));
        }
        windows_to_images.0.insert(window_e, image.clone());
        windows_to_images.0.insert(Entity::PLACEHOLDER, image);
    }
}

pub fn prepare_window_target_textures(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut windows_to_images: ResMut<WindowsToImages>,
    windows: Query<(Entity, &Window), Added<Window>>,
    cameras: Query<Entity, (Added<Camera>, With<TestCam>)>,
) {
    for (window_e, window) in windows.iter() {
        let image = images.add(new_render_target(
            window.physical_width(),
            window.physical_height(),
        ));
        // for cam in cameras.iter() {
        //     commands
        //         .entity(cam)
        //         .insert(Into::<RenderTarget>::into(image.clone()));
        // }
        windows_to_images.0.insert(window_e, image.clone());
        windows_to_images.0.insert(Entity::PLACEHOLDER, image);
    }
}

fn replace_window_targets_in_extracted_cameras(
    mut cameras: Query<(Entity, &mut ExtractedCamera)>,
    windows_to_images: Extract<Res<WindowsToImages>>,
    mut image_copiers: ResMut<ImageCopiers>,
    render_device: Res<RenderDevice>,
    images: Res<bevy::render::render_asset::RenderAssets<bevy::render::texture::GpuImage>>,
    // render_device: Res<RenderDevice>,
    // render_queue: Res<RenderQueue>,
    // default_sampler: Res<DefaultImageSampler>,
) {
    for (_camera_e, mut camera) in cameras.iter_mut() {
        let size = camera.physical_target_size.unwrap();
        if let Some(NormalizedRenderTarget::Image(..)) = &camera.target {
            let e = Entity::PLACEHOLDER;
            let copier = image_copiers.get(&e);
            let handle = windows_to_images.0[&e].clone();
            if copier.is_some_and(|copier| copier.src_image != handle) || copier.is_none() {
                let new_copier = ImageCopier::new(
                    handle,
                    Extent3d {
                        width: size.x,
                        height: size.y,
                        // width: 1280,
                        // height: 700,
                        depth_or_array_layers: 1,
                    },
                    &render_device,
                );
                image_copiers.insert(e, new_copier);
            }
        }
        let Some(NormalizedRenderTarget::Window(window_ref)) = &camera.target else {
            continue;
        };
        let window_e = window_ref.entity();
        if let Some(handle) = windows_to_images.0.get(&window_e).cloned() {
            camera.target = Some(NormalizedRenderTarget::Image(ImageRenderTarget {
                handle: handle.clone(),
                scale_factor: 1.0,
            }));
            camera.physical_viewport_size = uvec2(500, 500).into();
            camera.physical_target_size = uvec2(500, 500).into();
            let copier = image_copiers.get(&window_e);
            if copier.is_some_and(|copier| copier.src_image != handle) || copier.is_none() {
                let new_copier = ImageCopier::new(
                    handle,
                    Extent3d {
                        // width: size.x,
                        // height: size.y,
                        width: 500,
                        height: 500,
                        depth_or_array_layers: 1,
                    },
                    &render_device,
                );
                image_copiers.insert(window_e, new_copier);
            }
        }
    }
}

fn new_render_target(width: u32, height: u32) -> Image {
    let mut target = Image::new_uninit(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    // We're going to render to this image, mark it as such
    target.texture_descriptor.usage |= TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC;
    target
}

#[derive(Resource, Deref)]
struct RenderWorldSender(IpcSender<GameMsg>);

/// `ImageCopier` aggregator in `RenderWorld`
#[derive(Clone, Default, Resource, Deref, DerefMut, Debug)]
struct ImageCopiers(pub EntityHashMap<ImageCopier>);

/// Used by `ImageCopyDriver` for copying from render target to buffer
#[derive(Clone, Component, Debug)]
struct ImageCopier {
    buffer: Buffer,
    src_image: Handle<Image>,
}

impl ImageCopier {
    pub fn new(
        src_image: Handle<Image>,
        size: Extent3d,
        render_device: &RenderDevice,
    ) -> ImageCopier {
        let padded_bytes_per_row = RenderDevice::align_copy_bytes_per_row(size.width as usize * 4);
        let cpu_buffer = render_device.create_buffer(&BufferDescriptor {
            label: None,
            size: padded_bytes_per_row as u64 * size.height as u64,
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        ImageCopier {
            buffer: cpu_buffer,
            src_image,
        }
    }
}

// Copies image content from render target to buffer
fn image_copy_driver(
    render_context: RenderContext,
    image_copiers: Res<ImageCopiers>,
    render_queue: Res<RenderQueue>,
    gpu_images: Res<RenderAssets<GpuImage>>,
) {
    for image_copier in image_copiers.values() {
        let src_image = gpu_images.get(&image_copier.src_image).unwrap();

        let mut encoder = render_context
            .render_device()
            .create_command_encoder(&CommandEncoderDescriptor::default());

        let block_dimensions = src_image.texture_descriptor.format.block_dimensions();
        let block_size = src_image
            .texture_descriptor
            .format
            .block_copy_size(None)
            .unwrap();

        // Calculating correct size of image row because
        // copy_texture_to_buffer can copy image only by rows aligned wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
        // That's why image in buffer can be little bit wider
        // This should be taken into account at copy from buffer stage
        let padded_bytes_per_row = RenderDevice::align_copy_bytes_per_row(
            (src_image.texture_descriptor.size.width as usize / block_dimensions.0 as usize)
                * block_size as usize,
        );

        encoder.copy_texture_to_buffer(
            src_image.texture.as_image_copy(),
            TexelCopyBufferInfo {
                buffer: &image_copier.buffer,
                layout: TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(
                        std::num::NonZero::<u32>::new(padded_bytes_per_row as u32)
                            .unwrap()
                            .into(),
                    ),
                    rows_per_image: None,
                },
            },
            src_image.texture_descriptor.size,
        );

        render_queue.submit(std::iter::once(encoder.finish()));
    }
}

/// runs in render world after Render stage to send image from buffer via channel (receiver is in main world)
fn receive_image_from_buffer(
    image_copiers: Res<ImageCopiers>,
    render_device: Res<RenderDevice>,
    sender: Res<RenderWorldSender>,
) {
    for image_copier in image_copiers.values() {
        let buffer_slice = image_copier.buffer.slice(..);

        let (s, r) = std::sync::mpsc::sync_channel(1);
        // Maps the buffer so it can be read on the cpu
        buffer_slice.map_async(MapMode::Read, move |r| match r {
            // This will execute once the gpu is ready, so after the call to poll()
            Ok(r) => s.send(r).expect("Failed to send map update"),
            Err(err) => panic!("Failed to map buffer {err}"),
        });

        // In order for the mapping to be completed, one of three things must happen.
        // One of those can be calling `Device::poll`. This isn't necessary on the web as devices
        // are polled automatically but natively, we need to make sure this happens manually.
        // `Maintain::Wait` will cause the thread to wait on native but not on WebGpu.

        // This blocks until the gpu is done executing everything
        render_device
            .poll(PollType::wait_indefinitely())
            .expect("Failed to poll device for map async");

        // This blocks until the buffer is mapped
        r.recv().expect("Failed to receive the map_async message");

        sender
            .send(GameMsg::Image(buffer_slice.get_mapped_range().to_vec()))
            .unwrap();

        // We need to make sure all `BufferView`'s are dropped before we do what we're about
        // to do.
        // Unmap so that we can copy to the staging buffer in the next iteration.
        image_copier.buffer.unmap();
    }
}
