use bevy::{
    dev_tools::fps_overlay::FpsOverlayPlugin,
    diagnostic::FrameTimeDiagnosticsPlugin,
    feathers::{FeathersPlugins, controls::FeathersCheckbox},
    prelude::*,
    ui::Checked,
};
use editor_api::{GameProcess, GotSystems, ModifySystem, OutOfProcessPlugin};

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
        .add_systems(Startup, setup)
        .add_observer(
            |_on: On<GotSystems>, game: Res<GameProcess>, mut commands: Commands| {
                commands.spawn_scene(system_menu(game));
            },
        )
        .run()
}

fn setup(mut commands: Commands) {
    commands.spawn_scene_list(bsn_list! {
        Camera2d,
        Node {
            left: px(600),
        }
        Children [
            Text::new("\"Editor\" stub")
        ]
    });
}

fn system_menu(game: Res<GameProcess>) -> impl Scene {
    let systems_checkboxes = game
        .systems
        .keys()
        .map(|name| {
            let name = name.clone();
            bsn! {
                :FeathersCheckbox {
                    @caption: {bsn! { Text({name.clone()}) }}
                }
                Checked
                on(
                    move |change: On<bevy::ui_widgets::ValueChange<bool>>,
                        mut commands: Commands| {
                        let mut checkbox = commands.entity(change.source);
                        if change.value {
                            checkbox.insert(Checked);
                        } else {
                            checkbox.remove::<Checked>();
                        }
                        commands.trigger(ModifySystem{
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
            justify_self: JustifySelf::End

        }
        BackgroundColor(Color::BLACK)
        Children[
            Text("Enabled systems"),
            {systems_checkboxes}
        ]
    }
}
