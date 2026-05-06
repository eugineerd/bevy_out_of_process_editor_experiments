use bevy::prelude::*;
use editor_api::OutOfProcessPlugin;

fn main() -> AppExit {
    App::new()
        .add_plugins((DefaultPlugins, OutOfProcessPlugin))
        .run()
}
