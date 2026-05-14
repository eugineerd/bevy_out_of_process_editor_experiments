use bevy::asset::RenderAssetUsages;
use bevy::platform::collections::HashMap;
use bevy::platform::sync::Arc;
use bevy::platform::sync::Mutex;
use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use bevy::reflect::serde::{TypedReflectDeserializer, TypedReflectSerializer};
use bevy::render::extract_resource::ExtractResource;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::renderer::RenderDevice;
use bevy::render::texture::GpuImage;
use bevy::render::{Extract, Render, RenderApp, RenderSystems};
use bevy::window::WindowEvent;
use ipc_channel::ipc::{IpcOneShotServer, IpcSender};
use rustix::process::{Pid, PidfdFlags};
use serde::de::DeserializeSeed;
use serde::{Deserialize, Serialize};
use std::os::fd::OwnedFd;
use thiserror::Error;
use wgpu::TextureViewDescriptor;

mod external_texture;

#[derive(Default)]
pub struct OutOfProcessPlugin;

impl Plugin for OutOfProcessPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(start_game_process_observer)
            .init_resource::<ViewportTargets>()
            .add_observer(ViewportTextureUpdate::observer)
            .add_systems(PreUpdate, process_game_messages);
    }

    fn finish(&self, app: &mut App) {
        app.sub_app_mut(RenderApp)
            .add_systems(
                ExtractSchedule,
                |targets: Extract<Res<ViewportTargets>>, mut commands: Commands| {
                    commands.insert_resource(targets.clone());
                },
            )
            .add_systems(
                Render,
                (|targets: Res<ViewportTargets>,
                  mut gpu_images: ResMut<RenderAssets<GpuImage>>| {
                    for (k, v) in targets.values() {
                        if let Some(image) = gpu_images.get_mut(k) {
                            let texture: bevy::render::render_resource::Texture = v.clone().into();
                            let texture_view =
                                texture.create_view(&TextureViewDescriptor::default());
                            image.texture = texture;
                            image.texture_view = texture_view;
                        }
                    }
                })
                .in_set(RenderSystems::PrepareResources),
            );
    }
}

#[derive(Event)]
pub struct StartGameProcess {
    pub workspace_path: String,
}

fn start_game_process_observer(
    on: On<StartGameProcess>,
    mut commands: Commands,
    registry: Res<AppTypeRegistry>,
    game: Option<ResMut<GameProcess>>,
) {
    let (server, name) = IpcOneShotServer::<GameMsg>::new().unwrap();
    let game_proc = std::process::Command::new("cargo")
        .args(["run"])
        .current_dir(&on.workspace_path)
        .env(EDITOR_SERVER_NAME_VAR, name)
        .spawn()
        .unwrap();
    let (reciver, first_msg) = server.accept().unwrap();
    let GameMsg::Sender(sender) = first_msg else {
        panic!("Expected a Sender as a first message")
    };
    let msg_queue = Arc::new(Mutex::new(Vec::new()));
    std::thread::spawn({
        let msg_queue = msg_queue.clone();
        move || {
            loop {
                let msg = match reciver.recv() {
                    Ok(msg) => msg,
                    Err(err) => {
                        error!("{err}");
                        break;
                    }
                };
                let mut queue = msg_queue.lock().unwrap();
                queue.push(msg);
            }
        }
    });
    let game_proc = GameProcess {
        proc: game_proc,
        to_game: sender,
        registry: registry.clone(),
        msg_queue,
        systems: Default::default(),
    };
    if let Some(mut game) = game {
        game.proc.kill().unwrap();
        *game = game_proc;
    } else {
        commands.insert_resource(game_proc);
    }
}

pub struct RemoteSystem {
    pub is_running: bool,
}

#[derive(Resource)]
pub struct GameProcess {
    proc: std::process::Child,
    to_game: IpcSender<EditorMsg>,
    registry: AppTypeRegistry,
    msg_queue: Arc<Mutex<Vec<GameMsg>>>,
    pub systems: HashMap<String, RemoteSystem>,
}

impl GameProcess {
    pub fn send(&self, msg: EditorMsg) {
        if let Err(e) = self.to_game.send(msg) {
            error!("Sending faild: {e}");
        }
    }

    pub fn trigger(&self, event: impl Event + Reflect) {
        let event = ReflectedEvent::from_event(&self.registry.read(), event);
        self.send(event.into());
    }

    pub fn get_messages(&self, swap_to: &mut Vec<GameMsg>) {
        swap_to.clear();
        core::mem::swap(&mut *self.msg_queue.lock().unwrap(), swap_to);
    }
}

#[derive(Default, Resource, ExtractResource, Clone, DerefMut, Deref)]
pub struct ViewportTargets(HashMap<u64, (Handle<Image>, wgpu::Texture)>);

fn process_game_messages(
    mut commands: Commands,
    mut game: If<ResMut<GameProcess>>,
    mut msg_queue: Local<Vec<GameMsg>>,
    registry: Res<AppTypeRegistry>,
) {
    let status = game.proc.try_wait().unwrap();
    if status.is_some() {
        commands.remove_resource::<GameProcess>();
        return;
    }
    game.get_messages(&mut msg_queue);
    for msg in msg_queue.drain(..) {
        match msg {
            GameMsg::Image(info) => {
                info!("Got image: {info:?}");
                commands.trigger(ViewportTextureUpdate { info });
            }
            GameMsg::WorldInfo { systems } => {
                game.systems = systems
                    .into_iter()
                    .map(|n| (n, RemoteSystem { is_running: false }))
                    .collect();
                commands.trigger(GotSystems);
            }
            GameMsg::ReflectedEvent(event) => {
                let registry = registry.clone();
                commands.queue(move |world: &mut World| {
                    event.trigger(&registry.read(), world);
                });
            }
            _ => (),
        }
    }
}

#[derive(Event)]
pub struct GotSystems;

#[derive(Event)]
pub struct ViewportTextureUpdate {
    pub info: ExternalTextureInfo,
}

#[derive(Event)]
pub struct ViewportTextureCreated {
    pub id: u64,
}

impl ViewportTextureUpdate {
    pub fn observer(
        on: On<Self>,
        mut images: ResMut<Assets<Image>>,
        mut targets: ResMut<ViewportTargets>,
        device: Res<RenderDevice>,
        mut commands: Commands,
    ) {
        let Some(id) = on.info.texture_id else {
            error!("External texture must have an id");
            return;
        };
        let texture = match ExternalTexture::import(device.wgpu_device(), &on.info) {
            Ok(texture) => texture,
            Err(err) => {
                error!("{err}");
                return;
            }
        };
        if let Some((handle, existing_texture)) = targets.get_mut(&id)
            && let Some(mut existing_image) = images.get_mut(handle)
        {
            existing_image.resize(on.info.inner.wgpu_size);
            *existing_texture = texture;
        } else {
            let image = images.add(Image::new_fill(
                Extent3d {
                    width: on.info.inner.wgpu_size.width,
                    height: on.info.inner.wgpu_size.height,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                &[0, 0, 0, 255],
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::RENDER_WORLD,
            ));
            targets.insert(id, (image, texture));
            commands.trigger(ViewportTextureCreated { id })
        };
    }
}

#[derive(Event, Reflect, Serialize, Deserialize)]
#[reflect(Event, Serialize, Deserialize)]
pub struct ToggleSystemEvent {
    pub name: String,
    pub state: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReflectedEvent {
    id: String,
    data: Vec<u8>,
}

impl ReflectedEvent {
    pub fn from_event(registry: &TypeRegistry, event: impl Event + Reflect) -> Self {
        let type_path = event.reflect_type_path().to_string();
        let serializer = TypedReflectSerializer::new(event.as_partial_reflect(), registry);
        ReflectedEvent {
            id: type_path,
            data: postcard::to_allocvec(&serializer).unwrap(),
        }
    }

    pub fn trigger(self, registry: &TypeRegistry, world: &mut World) {
        let registration = registry.get_with_type_path(&self.id).unwrap();
        let reflect_event = registration.data::<ReflectEvent>().unwrap();
        let deserializer = TypedReflectDeserializer::new(registration, registry);
        let mut data = postcard::Deserializer::from_bytes(&self.data);
        let event = deserializer.deserialize(&mut data).unwrap();
        reflect_event.trigger(world, event.as_partial_reflect(), registry);
    }
}

impl From<ReflectedEvent> for EditorMsg {
    fn from(value: ReflectedEvent) -> Self {
        EditorMsg::ReflectedEvent(value)
    }
}

pub const EDITOR_SERVER_NAME_VAR: &'static str = "BEVY_EDITOR_SERVER_NAME";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EditorMsg {
    NextFrame, // TODO: implement rendering synchronization
    Exit,
    Pause,
    Continue,
    WindowEvent(WindowEvent),
    ReflectedEvent(ReflectedEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GameMsg {
    Sender(IpcSender<EditorMsg>),
    Image(ExternalTextureInfo),
    WorldInfo { systems: Vec<String> },
    ReflectedEvent(ReflectedEvent),
}

#[derive(Error, Debug)]
pub enum ExternalTextureImportError {
    #[error("Io error: {0}")]
    Io(#[from] rustix::io::Errno),
    #[error("Invalid pid value in external texture info")]
    InvalidPid,
    #[error("Texture share error: {0}")]
    TextureShareError(#[from] external_texture::TextureShareError),
}

pub struct ExternalTexture {
    texture: wgpu::Texture,
    info: ExternalTextureInfo,
    _handle: OwnedFd,
}

impl ExternalTexture {
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    pub fn info(&self) -> &ExternalTextureInfo {
        &self.info
    }

    pub fn new(
        device: &wgpu::Device,
        desc: &wgpu::TextureDescriptor,
        id: Option<u64>,
    ) -> Result<Self> {
        let owner_pid = std::process::id();
        unsafe {
            let texture = external_texture::create_exportable_texture(device, desc).unwrap();
            let (fd, meta) = external_texture::export_texture(device, &texture).unwrap();
            Ok(ExternalTexture {
                texture,
                info: ExternalTextureInfo {
                    inner: meta,
                    texture_id: id,
                    owner_pid,
                },
                _handle: fd,
            })
        }
    }

    pub fn import(
        device: &wgpu::Device,
        info: &ExternalTextureInfo,
    ) -> Result<wgpu::Texture, ExternalTextureImportError> {
        let owner_pid =
            Pid::from_raw(info.owner_pid as i32).ok_or(ExternalTextureImportError::InvalidPid)?;
        let pidfd = rustix::process::pidfd_open(owner_pid, PidfdFlags::empty())?;
        let image_fd = external_texture::steal_fd_via_pidfd(&pidfd, info.inner.image_fd)?;
        Ok(unsafe { external_texture::import_texture(device, image_fd, &info.inner)? })
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ExternalTextureInfo {
    owner_pid: u32,
    pub texture_id: Option<u64>,
    inner: external_texture::TextureMetadata,
}
