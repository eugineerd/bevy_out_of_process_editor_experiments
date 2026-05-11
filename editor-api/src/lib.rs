use std::os::fd::{OwnedFd, RawFd};

use bevy::image::ImageSampler;
use bevy::platform::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::input::common_conditions;
use bevy::platform::collections::HashMap;
use bevy::platform::sync::Mutex;
use bevy::remote::builtin_methods::{
    BRP_DESPAWN_COMPONENTS_METHOD, BRP_GET_COMPONENTS_METHOD, BRP_LIST_COMPONENTS_METHOD,
    BRP_QUERY_METHOD, BrpGetComponentsParams, BrpGetComponentsResponse, BrpListComponentsParams,
    BrpListComponentsResponse, BrpQueryParams, BrpQueryResponse,
};
use bevy::remote::{BrpPayload, BrpRequest};
use bevy::render::extract_resource::ExtractResource;
use bevy::render::render_asset::{RenderAsset, RenderAssets};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::texture::{DefaultImageSampler, GpuImage};
use bevy::render::{Extract, Render, RenderApp, RenderSystems};
use bevy::sprite_render::Material2d;
use bevy::ui_render::extract_uinode_images;
use bevy::window::{PrimaryWindow, WindowEvent};
use bevy::{prelude::*, remote::builtin_methods::BrpDespawnEntityParams};
use ipc_channel::ipc::{IpcOneShotServer, IpcSender};
use rustix::process::{Pid, PidfdFlags};
use serde::de::{DeserializeOwned, DeserializeSeed};
use serde::{Deserialize, Serialize};

mod external_texture;
mod ipc;
mod server_side;
pub use server_side::*;
use wgpu::TextureViewDescriptor;

#[derive(Default)]
pub struct OutOfProcessPlugin;

impl Plugin for OutOfProcessPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(start_game_process_observer)
            .init_resource::<ImagesS>()
            .add_systems(
                Update,
                (|mut commands: Commands| {
                    commands.trigger(StartGameProcess {});
                })
                .run_if(common_conditions::input_just_pressed(KeyCode::KeyU)),
            )
            .add_systems(PreUpdate, sync_world);
    }

    fn finish(&self, app: &mut App) {
        app.sub_app_mut(RenderApp)
            .add_systems(
                ExtractSchedule,
                |imagess: Extract<Res<ImagesS>>, mut commands: Commands| {
                    commands.insert_resource(imagess.clone());
                },
            )
            .add_systems(
                Render,
                (|imagess: Res<ImagesS>, mut gpu_images: ResMut<RenderAssets<GpuImage>>| {
                    for (k, v) in imagess.0.iter() {
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
pub struct StartGameProcess {}

fn start_game_process_observer(
    _on: On<StartGameProcess>,
    mut commands: Commands,
    game: Option<ResMut<GameProcess>>,
) {
    let path = "/workspaces/bevy-editor-experiments";
    let (server, name) = IpcOneShotServer::<GameMsg>::new().unwrap();
    let game_proc = std::process::Command::new("cargo")
        .args(["run", "-p", "game"])
        .current_dir(path)
        .env(EDITOR_SERVER_NAME_VAR, name)
        .spawn()
        .unwrap();
    let (reciver, first_msg) = server.accept().unwrap();
    let GameMsg::Sender(sender) = first_msg else {
        panic!("Not Sender")
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
        initialized: false,
        entities_map: Default::default(),
        reverse_entities_map: Default::default(),
        proc: game_proc,
        to_game: sender,
        // from_game: Mutex::new(reciver),
        msg_queue,
    };
    if let Some(mut game) = game {
        game.proc.kill().unwrap();
        *game = game_proc;
    } else {
        commands.insert_resource(game_proc);
    }
}

#[derive(Resource)]
pub struct GameProcess {
    initialized: bool,
    entities_map: HashMap<Entity, Entity>,
    reverse_entities_map: HashMap<Entity, Entity>,
    proc: std::process::Child,
    to_game: IpcSender<EditorMsg>,
    // from_game: Mutex<IpcReceiver<GameMsg>>,
    msg_queue: Arc<Mutex<Vec<GameMsg>>>,
}

/// A response according to BRP.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BrpResponse {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    #[serde(flatten)]
    pub payload: BrpPayload,
}

fn request<R: DeserializeOwned + core::fmt::Debug, T: Serialize + core::fmt::Debug>(
    v: T,
    method: &str,
) -> R {
    // info!("req: {:?}", &v);
    let body = BrpRequest {
        id: None,
        method: method.into(),
        params: serde_json::to_value(v).unwrap().into(),
    };
    todo!();
    // let req = Request::post("http://127.0.0.1:15702", serde_json::to_vec(&body).unwrap());
    // let resp = ehttp::fetch_blocking(&req).unwrap();
    // let resp_value = serde_json::from_slice::<BrpResponse>(&resp.bytes).unwrap();
    // match resp_value.payload {
    //     bevy::remote::BrpPayload::Result(value) => {
    //         let response_value = serde_json::from_value::<R>(value).unwrap();
    //         // info!("resp: {:?}", &response_value);
    //         return response_value;
    //     }
    //     bevy::remote::BrpPayload::Error(err) => panic!("{}", err.message),
    // };
}

fn reset_scene() {
    let resp: BrpQueryResponse = request(
        BrpQueryParams {
            data: bevy::remote::builtin_methods::BrpQuery::default(),
            filter: bevy::remote::builtin_methods::BrpQueryFilter {
                with: vec![SceneEntity::type_path().into()],
                ..Default::default()
            },
            strict: false,
        },
        BRP_QUERY_METHOD,
    );
    for row in resp {
        let _: () = request(
            BrpDespawnEntityParams { entity: row.entity },
            BRP_DESPAWN_COMPONENTS_METHOD,
        );
    }
}

fn spawn_editor_sync(world: &mut World, game: &mut GameProcess) {
    let resp: BrpQueryResponse = request(
        BrpQueryParams {
            data: bevy::remote::builtin_methods::BrpQuery::default(),
            filter: bevy::remote::builtin_methods::BrpQueryFilter {
                with: vec![EditorSync::type_path().into()],
                ..Default::default()
            },
            strict: false,
        },
        BRP_QUERY_METHOD,
    );

    fn sync_entity(
        game_entity: Entity,
        entities_map: &mut HashMap<Entity, Entity>,
        world: &mut World,
    ) {
        // let registry = world.resource::<AppTypeRegistry>().clone();
        let had_entity = entities_map.contains_key(&game_entity);
        let editor_entity = entities_map
            .entry(game_entity)
            .or_insert_with(|| world.spawn_empty().id())
            .clone();
        if !had_entity {
            info!("Spawned {editor_entity}");
        }
        let resp: BrpListComponentsResponse = request(
            BrpListComponentsParams {
                entity: game_entity,
            },
            BRP_LIST_COMPONENTS_METHOD,
        );
        let resp: BrpGetComponentsResponse = request(
            BrpGetComponentsParams {
                entity: game_entity,
                strict: false,
                components: resp,
            },
            BRP_GET_COMPONENTS_METHOD,
        );
        let BrpGetComponentsResponse::Lenient {
            mut components,
            errors,
        } = resp
        else {
            panic!("Not lenient response?");
        };
        if !errors.is_empty() {
            info!("{:?}", errors)
        }
        let children = components.remove(Children::type_path());
        if let Some(child_of_raw) = components.remove(ChildOf::type_path()) {
            let child_of: ChildOf = serde_json::from_value(child_of_raw.clone()).unwrap();
            let editor_child_of = entities_map[&child_of.0];
            world.entity_mut(editor_child_of).add_child(editor_entity);
        }
        components.remove(bevy::scene::SceneComponentInfo::type_path());

        let app_type_registry = world.resource::<AppTypeRegistry>().clone();
        let type_registry = app_type_registry.read();
        let mut scratch = bevy::ecs::bundle::BundleScratch::default();
        let mut writer = scratch.writer();
        for (component_path, component) in components {
            let Some(component_type) = type_registry.get_with_type_path(&component_path) else {
                continue;
            };
            let type_id = component_type.type_id();
            let reflect_from_reflect = type_registry
                .get_type_data::<ReflectFromReflect>(type_id)
                .unwrap();
            let reflect_component = type_registry
                .get_type_data::<ReflectComponent>(type_id)
                .unwrap();
            let component_id = reflect_component.register_component(world);
            let layout = world.components().get_info(component_id).unwrap().layout();
            let reflected =
                bevy::reflect::serde::TypedReflectDeserializer::new(component_type, &type_registry)
                    .deserialize(&component)
                    .unwrap();
            let value = reflect_from_reflect
                .from_reflect(reflected.as_partial_reflect())
                .unwrap();
            let value_ptr = std::ptr::NonNull::new(Box::into_raw(value).cast::<u8>()).unwrap();
            unsafe {
                writer.push_component_by_id(
                    component_id,
                    bevy::ptr::OwningPtr::new(value_ptr),
                    layout,
                );
            }
        }
        unsafe {
            writer.write(&mut world.entity_mut(editor_entity));
        }
        if let Some(children_raw) = children {
            let children: Vec<Entity> = serde_json::from_value(children_raw).unwrap();
            for child in children {
                sync_entity(child, entities_map, world);
            }
        }
    }
    for row in resp {
        let game_entity = row.entity;
        sync_entity(game_entity, &mut game.entities_map, world);
    }
    game.reverse_entities_map = game.entities_map.iter().map(|(k, v)| (*v, *k)).collect();
}

#[derive(Default, Resource, ExtractResource, Clone)]
struct ImagesS(HashMap<Handle<Image>, wgpu::Texture>);

fn sync_world(world: &mut World) {
    if !world.contains_resource::<GameProcess>() {
        return;
    }
    world
        .run_system_cached(
            move |mut commands: Commands,
                  mut game: ResMut<GameProcess>,
                  mut msgs: MessageReader<WindowEvent>,
                  mut images: ResMut<Assets<Image>>,
                  mut sprites: Query<&mut Sprite>,
                  //   window: Single<&Window, With<PrimaryWindow>>,
                  mut imagess: ResMut<ImagesS>,
                  mut msg_queue: Local<Vec<GameMsg>>,
                  device: Res<RenderDevice>,
                  keys: Res<ButtonInput<KeyCode>>| {
                game.to_game.send(EditorMsg::NextFrame).unwrap();
                let game = &mut *game;
                core::mem::swap(&mut *game.msg_queue.lock().unwrap(), &mut msg_queue);
                for msg in msg_queue.drain(..) {
                    match msg {
                        GameMsg::Image(meta) => {
                            info!("Got image: {meta:?}");
                            let game_pid = Pid::from_child(&game.proc);
                            let game_fd =
                                rustix::process::pidfd_open(game_pid, PidfdFlags::empty()).unwrap();
                            let fd = crate::external_texture::steal_fd_via_pidfd(
                                &game_fd,
                                meta.image_fd,
                            )
                            .unwrap();
                            let texture = unsafe {
                                crate::external_texture::import_texture(
                                    device.wgpu_device(),
                                    fd,
                                    &meta,
                                )
                                .unwrap()
                            };
                            let image = images.add(Image::new_fill(
                                Extent3d {
                                    width: meta.wgpu_size.width,
                                    height: meta.wgpu_size.height,
                                    depth_or_array_layers: 1,
                                },
                                TextureDimension::D2,
                                &[0, 0, 0, 255],
                                TextureFormat::Rgba8UnormSrgb,
                                RenderAssetUsages::RENDER_WORLD,
                            ));
                            imagess.0.insert(image.clone(), texture);

                            commands.spawn(Sprite {
                                image: image,
                                ..Default::default()
                            });
                            // if !game.initialized {
                            //     game.initialized = true;
                            // } else {
                            //     let mut sprite = sprites.single_mut().unwrap();
                            //     images.remove(&sprite.image);
                            //     sprite.image = image;
                            // }
                        }
                        _ => (),
                    }
                }
                for msg in msgs.read() {
                    game.to_game
                        .send(EditorMsg::WindowEvent(msg.clone()))
                        .unwrap();
                }
                if keys.just_pressed(KeyCode::KeyP) {
                    game.to_game.send(EditorMsg::Pause).unwrap();
                }
                if keys.just_pressed(KeyCode::KeyO) {
                    game.to_game.send(EditorMsg::Continue).unwrap();
                }
            },
        )
        .unwrap();
}

/*
pub fn update_scene(ast: &mut SceneJsnAst) {
    for &entity_idx in &ast.dirty_indices {
        let entity_jsn = &ast.nodes[entity_idx];
        if let Some(existing_entity) = entity_jsn.remote_entity {
            let _: () = request(
                BrpDespawnEntityParams {
                    entity: existing_entity,
                },
                BRP_DESPAWN_COMPONENTS_METHOD,
            );
        }
        let mut body =
            BrpSpawnEntityParams {
                components: HashMap::from_iter(entity_jsn.components.iter().filter_map(
                    |(k, v)| (!k.contains("jackdaw")).then_some((k.clone(), v.clone())),
                )),
            };
        if let Some(parent) = entity_jsn
            .parent
            .and_then(|parent| ast.nodes.get(parent))
            .and_then(|entry| entry.remote_entity)
        {
            body.components
                .insert(ChildOf::type_path().into(), Value::from(parent.to_bits()));
        }
        body.components.insert(
            SceneEntity::type_path().into(),
            serde_json::to_value(SceneEntity {}).unwrap(),
        );
        let resp: BrpSpawnEntityResponse = request(body, BRP_SPAWN_ENTITY_METHOD);
        ast.nodes[entity_idx].remote_entity = Some(resp.entity);
    }
    ast.dirty_indices.clear();
}
 */

pub const EDITOR_SERVER_NAME_VAR: &'static str = "BEVY_EDITOR_SERVER_NAME";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EditorMsg {
    NextFrame,
    WindowEvent(WindowEvent),
    Exit,
    Pause,
    Continue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GameMsg {
    Sender(IpcSender<EditorMsg>),
    Image(crate::external_texture::TextureMetadata),
}
