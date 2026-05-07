use bevy::{feathers::FeathersPlugins, prelude::*};
use editor_api::OutOfProcessPlugin;

fn main() -> AppExit {
    App::new()
        .insert_resource(bevy::feathers::theme::UiTheme(
            bevy::feathers::dark_theme::create_dark_theme(),
        ))
        .add_plugins((DefaultPlugins, OutOfProcessPlugin, FeathersPlugins))
        .add_systems(Startup, setup)
        .run()
}

fn setup(mut commands: Commands) {
    commands.spawn_scene(bsn! {
        Camera2d
    });
}
