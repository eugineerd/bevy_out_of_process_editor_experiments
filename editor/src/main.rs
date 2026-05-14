use bevy::{
    dev_tools::fps_overlay::FpsOverlayPlugin,
    diagnostic::FrameTimeDiagnosticsPlugin,
    feathers::{FeathersPlugins, controls::FeathersCheckbox},
    prelude::*,
    ui::Checked,
    window::WindowEvent,
};
use editor_common::{
    GameProcess, GotSystems, OutOfProcessPlugin, ToggleSystem, ViewportTargets,
    ViewportTextureCreated,
};

fn main() -> AppExit {
    App::new()
        .insert_resource(bevy::feathers::theme::UiTheme(
            bevy::feathers::dark_theme::create_dark_theme(),
        ))
        .add_plugins((
            DefaultPlugins,
            OutOfProcessPlugin,
            FeathersPlugins,
            FrameTimeDiagnosticsPlugin::default(),
            FpsOverlayPlugin::default(),
        ))
        .add_systems(PostStartup, setup)
        .add_systems(Update, send_events_to_game_viewport)
        .add_observer(
            |_on: On<GotSystems>,
             game: Res<GameProcess>,
             mut commands: Commands,
             panel: Query<Entity, With<Panel>>|
             -> Result {
                let panel_e = panel.single()?;
                commands.entity(panel_e).despawn_children();
                commands.spawn_scene(bsn! {:system_menu(game) ChildOf(panel_e)});
                Ok(())
            },
        )
        .add_observer(update_game_viewport_texture_observer)
        .run()
}

fn setup(mut commands: Commands) {
    commands.spawn_scene_list(bsn_list! {
        Camera2d,
        Node {
            display: Display::Flex,
            width: percent(100),
            max_width: percent(100)
            height: percent(100),
            max_height: percent(100),
            flex_grow: 1.0,
            flex_shrink: 1.0,
        }
        Children [
            Node {
                min_width: px(400),
            }
            Panel
            BackgroundColor(Color::BLACK),
            :GameViewport,
        ]
    });
}

#[derive(Component, Clone, Default)]
struct Panel;

fn system_menu(game: Res<GameProcess>) -> impl Scene {
    let mut systems_checkboxes = game.systems.keys().cloned().collect::<Vec<_>>();
    systems_checkboxes.sort_unstable();
    let systems_checkboxes = systems_checkboxes
        .into_iter()
        .map(|name| {
            bsn! {
                :FeathersCheckbox {
                    @caption: {
                        bsn! {
                            TextFont{
                                font_size: px(10)
                            }
                            Text({name.clone()})
                        }
                    }
                }
                Checked
                on(
                    move |change: On<bevy::ui_widgets::ValueChange<bool>>,
                        mut commands: Commands,
                        game: Res<GameProcess> | {
                        let mut checkbox = commands.entity(change.source);
                        if change.value {
                            checkbox.insert(Checked);
                        } else {
                            checkbox.remove::<Checked>();
                        }
                        game.trigger(ToggleSystem {
                            name: name.clone(),
                            state: change.value
                        })
                    }
                )
            }
        })
        .collect::<Vec<_>>();
    bsn! {
        Node {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            align_self: AlignSelf::End,
            justify_self: JustifySelf::End,
            max_width: px(400),
            max_height: px(800),
            overflow: Overflow::scroll_y(),
            scrollbar_width: 20.0,
        }
        ScrollPosition
        Pickable
        on(|on: On<Pointer<Scroll>>, mut scroll_pos: Query<&mut ScrollPosition>| -> Result{
            scroll_pos.get_mut(on.entity)?.y -= on.y * 20.0;
            Ok(())
        })
        Children[
            Text("Enabled systems")
            ,
            {systems_checkboxes}
        ]
    }
}

#[derive(SceneComponent, FromTemplate)]
pub struct GameViewport {
    focused: bool,
    id: Option<u64>,
    image: Entity,
}

impl GameViewport {
    pub fn scene() -> impl Scene {
        bsn! {
            GameViewport{
                image: #Image
            }
            Node {
                width: percent(100),
                height: percent(100),
                flex_shrink: 1.0,
            }
            Pickable
            on(|on: On<Pointer<Enter>>, mut viewport: Query<&mut GameViewport>| -> Result {
                viewport.get_mut(on.entity)?.focused = true;
                Ok(())
            })
            on(|on: On<Pointer<Leave>>, mut viewport: Query<&mut GameViewport>| -> Result {
                viewport.get_mut(on.entity)?.focused = false;
                Ok(())
            })
            Children [
                #Image
                Node {
                    position_type: PositionType::Absolute
                }
                ImageNode
            ]
        }
    }
}

pub fn update_game_viewport_texture_observer(
    on: On<ViewportTextureCreated>,
    targets: Res<ViewportTargets>,
    mut viewport: Query<(&mut GameViewport, &ComputedNode)>,
    mut image_nodes: Query<&mut ImageNode>,
) {
    if let Some((handle, ..)) = targets.get(&on.id)
        && let Some((mut viewport, _computed_node)) = viewport.iter_mut().next()
        && let Ok(mut viewport_image) = image_nodes.get_mut(viewport.image)
    {
        viewport_image.image = handle.clone();
        viewport.id = Some(on.id);
    }
}

pub fn send_events_to_game_viewport(
    viewports: Query<(&GameViewport, Ref<ComputedNode>, &UiGlobalTransform)>,
    game_proc: If<Res<GameProcess>>,
    mut window_events: MessageReader<WindowEvent>,
) {
    for (viewport, computed_node, tr) in viewports {
        let Some(window_e) = viewport.id.and_then(|id| Entity::try_from_bits(id)) else {
            continue;
        };

        if computed_node.is_changed() {
            game_proc.send(editor_common::EditorMsg::WindowEvent(
                WindowEvent::WindowResized(bevy::window::WindowResized {
                    window: window_e,
                    width: computed_node.size().x as f32,
                    height: computed_node.size().y as f32,
                }),
            ));
        }

        if !viewport.focused {
            continue;
        }
        let top_left = tr.translation - (computed_node.size / 2.0);

        for event in window_events.read() {
            let mut event = event.clone();
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
                WindowEvent::WindowBackendScaleFactorChanged(
                    window_backend_scale_factor_changed,
                ) => window_backend_scale_factor_changed.window = window_e,
                WindowEvent::WindowCloseRequested(window_close_requested) => {
                    window_close_requested.window = window_e
                }
                WindowEvent::WindowCreated(window_created) => window_created.window = window_e,
                WindowEvent::WindowDestroyed(window_destroyed) => {
                    window_destroyed.window = window_e
                }
                WindowEvent::WindowFocused(window_focused) => window_focused.window = window_e,
                WindowEvent::WindowMoved(window_moved) => window_moved.window = window_e,
                WindowEvent::WindowOccluded(window_occluded) => window_occluded.window = window_e,
                WindowEvent::WindowResized(_window_resized) => continue,
                WindowEvent::WindowScaleFactorChanged(window_scale_factor_changed) => {
                    window_scale_factor_changed.window = window_e
                }
                WindowEvent::WindowThemeChanged(window_theme_changed) => {
                    window_theme_changed.window = window_e
                }
                WindowEvent::CursorMoved(cursor_moved) => {
                    cursor_moved.position.x -= top_left.x;
                    cursor_moved.position.y -= top_left.y;
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
                WindowEvent::KeyboardInput(keyboard_input) => {
                    keyboard_input.window = window_e;
                }
                WindowEvent::KeyboardFocusLost(..) => (),
            };

            game_proc.send(editor_common::EditorMsg::WindowEvent(event));
        }
    }
}
