use bevy::camera_controller::free_camera::FreeCameraPlugin;
use bevy::prelude::*;
use editor_api::EditorIntegrationPlugin;

#[derive(Component)]
pub struct DebugView;
mod feathers;
mod utils;

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins
                .build()
                .disable::<bevy::winit::WinitPlugin>()
                .disable::<bevy::audio::AudioPlugin>(),
            EditorIntegrationPlugin::default(),
            FreeCameraPlugin,
            utils::plugin,
            feathers::plugin,
        ))
        .add_systems(Startup, hello_world_system)
        .add_observer(|on: On<Add, Transform>, mut commands: Commands| {
            commands.entity(on.entity).insert(DebugView);
        })
        .add_systems(Update, (show, circle))
        .run();
}

fn show(mut gizmos: Gizmos, q: Query<&Transform, (With<DebugView>, Without<Camera>)>) {
    for t in q {
        gizmos.sphere(t.to_isometry(), 20.0, Color::WHITE);
    }
}

fn hello_world_system(mut commands: Commands) {
    commands.spawn((
        Camera2d::default(),
        Camera {
            order: 1,
            ..Default::default()
        },
        // bevy::camera_controller::free_camera::FreeCamera::default(),
    ));
    commands.spawn((Camera2d::default(), IsDefaultUiCamera));
}

fn circle(
    mut gizmos: Gizmos,
    cursor_pos: Res<utils::CursorPos>,
    // mut m: MessageReader<bevy::input::keyboard::KeyboardInput>,
) {
    // if !m.is_empty() {
    //     info!("{:?}", m.read().collect::<Vec<_>>())
    // }
    gizmos.circle_2d(
        cursor_pos.world_pos,
        cursor_pos
            .window_normalized_pos
            .x
            .remap(0.0, 1.0, 10.0, 10.0),
        Color::linear_rgb(1.0, 1.0, 0.5),
    );
}
