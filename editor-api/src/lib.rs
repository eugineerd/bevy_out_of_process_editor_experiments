use bevy::platform::collections::HashMap;
use bevy::remote::RemotePlugin;
use bevy::remote::builtin_methods::{
    BRP_DESPAWN_COMPONENTS_METHOD, BRP_GET_COMPONENTS_METHOD, BRP_LIST_COMPONENTS_METHOD,
    BRP_QUERY_METHOD, BRP_SPAWN_ENTITY_METHOD, BrpGetComponentsParams, BrpGetComponentsResponse,
    BrpInsertComponentsParams, BrpListComponentsParams, BrpListComponentsResponse, BrpQueryParams,
    BrpQueryResponse, BrpSpawnEntityResponse, process_remote_insert_components_request,
};
use bevy::remote::http::RemoteHttpPlugin;
use bevy::remote::{BrpPayload, BrpRequest};
use bevy::ui_widgets::Activate;
use bevy::window::{PrimaryWindow, WindowEvent};
use bevy::{
    prelude::*,
    remote::builtin_methods::{BrpDespawnEntityParams, BrpSpawnEntityParams},
};
use ehttp::Request;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Default)]
pub struct EditorIntegrationPlugin;

impl Plugin for EditorIntegrationPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
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
        ));
    }

    fn cleanup(&self, _app: &mut App) {
        // TODO: modify schedules to be stop user code from execution
    }
}

#[derive(Component, Reflect, Serialize, Deserialize)]
#[reflect(Component)]
pub struct EditorSync {}

#[derive(Component, Reflect, Serialize, Deserialize)]
#[reflect(Component)]
#[require(Button)]
pub struct EditorBtn {}

#[derive(Component, Reflect, Serialize, Deserialize)]
#[reflect(Component)]
pub struct SceneEntity {}

#[derive(Default)]
pub struct OutOfProcessPlugin;

impl Plugin for OutOfProcessPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameProcess>()
            .add_observer(
                |activate: On<Activate>,
                 q: Query<Entity, With<EditorSync>>,
                 game: Res<GameProcess>| {
                    info!("Listening to activate");
                    if !q.contains(activate.entity) {
                        return;
                    }
                    let Some(target_entity) =
                        game.reverse_entities_map.get(&activate.entity).copied()
                    else {
                        info!("Entity not found");
                        return;
                    };
                    let _: () = request(
                        BrpTriggerActivateEvent {
                            entity: target_entity,
                        },
                        BRP_TRIGGER_ACTIVATE_EVENT_METHOD,
                    );
                    info!("sent");
                },
            )
            .add_observer(
                |activate: On<Pointer<Click>>,
                 q: Query<Entity, With<EditorSync>>,
                 game: Res<GameProcess>| {
                    info!("Listening to click");
                    if !q.contains(activate.entity) {
                        return;
                    }
                    let Some(target_entity) =
                        game.reverse_entities_map.get(&activate.entity).copied()
                    else {
                        info!("Entity not found");
                        return;
                    };
                    let _: () = request(
                        BrpTriggerActivateEvent {
                            entity: target_entity,
                        },
                        BRP_TRIGGER_ACTIVATE_EVENT_METHOD,
                    );
                    info!("sent");
                },
            )
            .add_systems(PostUpdate, sync_world);
    }
}

pub fn launch_game_process() {
    std::thread::spawn(|| {
        let workdir = "/home/vscode/Projects/my_game";
        std::process::Command::new("cargo build")
            .current_dir(workdir)
            .spawn()
            .unwrap()
            .wait()
            .unwrap();
        std::process::Command::new("/home/vscode/Projects/my_game/target/debug/my_game")
            .current_dir(workdir)
            .spawn()
            .unwrap()
            .wait()
            .unwrap();
    });
}

#[derive(Resource, Default)]
struct GameProcess {
    initialized: bool,
    entities_map: HashMap<Entity, Entity>,
    reverse_entities_map: HashMap<Entity, Entity>,
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
    let req = Request::post("http://127.0.0.1:15702", serde_json::to_vec(&body).unwrap());
    let resp = ehttp::fetch_blocking(&req).unwrap();
    let resp_value = serde_json::from_slice::<BrpResponse>(&resp.bytes).unwrap();
    match resp_value.payload {
        bevy::remote::BrpPayload::Result(value) => {
            let response_value = serde_json::from_value::<R>(value).unwrap();
            // info!("resp: {:?}", &response_value);
            return response_value;
        }
        bevy::remote::BrpPayload::Error(err) => panic!("{}", err.message),
    };
}

const BRP_SEND_WINDOW_MESSAGE_METHOD: &str = "world.send_window_message";

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
struct BrpSendWindowMessageParams {
    messages: Vec<WindowEvent>,
}

fn process_window_message_request(
    In(params): In<Option<Value>>,
    mut message_writer: MessageWriter<WindowEvent>,
    window: Single<&mut Window, With<PrimaryWindow>>,
) -> bevy::remote::BrpResult {
    let BrpSendWindowMessageParams { messages } =
        bevy::remote::builtin_methods::parse_some(params)?;
    let mut window = window.into_inner();
    // window.focused = true;

    message_writer.write_batch(messages.into_iter().inspect(|f| match f {
        WindowEvent::CursorMoved(c) => window.set_physical_cursor_position(Some(c.position.into())),
        _ => (),
    }));

    Ok(().into())
}

const BRP_TRIGGER_ACTIVATE_EVENT_METHOD: &str = "world.trigger_activate_event";

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
struct BrpTriggerActivateEvent {
    entity: Entity,
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

fn spawn_editor_sync(world: &mut World) {
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
        if let Some(children_raw) = components.remove(Children::type_path()) {
            let children: Vec<Entity> = serde_json::from_value(children_raw).unwrap();
            for child in children {
                sync_entity(child, entities_map, world);
            }
        }
        if let Some(child_of_raw) = components.remove(ChildOf::type_path()) {
            let child_of: ChildOf = serde_json::from_value(child_of_raw.clone()).unwrap();
            let editor_child_of = entities_map[&child_of.0];
            world.entity_mut(editor_entity).add_child(editor_child_of);
        }
        world
            .run_system_cached_with(
                process_remote_insert_components_request,
                Some(
                    serde_json::to_value(BrpInsertComponentsParams {
                        entity: editor_entity,
                        components: components,
                    })
                    .unwrap(),
                ),
            )
            .unwrap()
            .unwrap();
    }
    let mut entities_map = core::mem::take(&mut world.resource_mut::<GameProcess>().entities_map);
    for row in resp {
        let game_entity = row.entity;
        sync_entity(game_entity, &mut entities_map, world);
    }
    let mut gp = world.resource_mut::<GameProcess>();
    gp.entities_map = entities_map;
    gp.reverse_entities_map = gp.entities_map.iter().map(|(k, v)| (*v, *k)).collect();
}

fn sync_world(world: &mut World) {
    // update_scene(&mut world.resource_mut::<SceneJsnAst>());
    let mut game = world.resource_mut::<GameProcess>();
    if !game.initialized {
        game.initialized = true;
        reset_scene();
        spawn_editor_sync(world);
    }
    // let registry = world.resource::<AppTypeRegistry>().clone();
    // let registry = registry.read();
    let window_messages = world.resource::<Messages<WindowEvent>>();
    let messages: Vec<_> = window_messages
        .iter_current_update_messages()
        .cloned()
        .collect();
    if !messages.is_empty() {
        let _: () = request(
            BrpSendWindowMessageParams { messages },
            BRP_SEND_WINDOW_MESSAGE_METHOD,
        );
    }
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
