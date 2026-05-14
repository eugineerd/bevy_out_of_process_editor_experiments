use bevy::camera_controller::free_camera::FreeCameraPlugin;
use bevy::dev_tools::fps_overlay::FpsOverlayPlugin;
use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::feathers::FeathersPlugins;
use bevy::prelude::*;
use editor_integration::EditorIntegrationPlugin;

#[derive(Component)]
pub struct DebugView;
mod feathers;
mod utils;

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins.build().disable::<bevy::audio::AudioPlugin>(),
            EditorIntegrationPlugin::default(),
            FeathersPlugins,
            // FrameTimeDiagnosticsPlugin::default(),
            // FpsOverlayPlugin::default(),
            FreeCameraPlugin,
            utils::plugin,
            feathers::plugin,
        ))
        .add_systems(Startup, hello_world_system)
        .add_systems(Update, |mut q: Query<&mut Transform, With<Sprite>>| {
            q.single_mut().unwrap().rotate_z(0.1);
        })
        .add_systems(Update, circle)
        .run();
}

fn hello_world_system(mut commands: Commands) {
    commands.spawn((
        Camera2d::default(),
        Camera {
            order: 1,
            ..Default::default()
        },
    ));
    commands.spawn((Camera2d::default(), IsDefaultUiCamera));
    commands.spawn((
        Sprite {
            color: Color::BLACK,
            custom_size: vec2(50.0, 50.0).into(),

            ..Default::default()
        },
        Transform::from_translation(vec3(100.0, 50.0, 0.0)),
    ));
}

fn circle(mut gizmos: Gizmos, cursor_pos: Res<utils::CursorPos>) {
    gizmos.circle_2d(
        cursor_pos.world_pos,
        cursor_pos
            .window_normalized_pos
            .x
            .remap(0.0, 1.0, 10.0, 10.0),
        Color::linear_rgb(1.0, 1.0, 0.5),
    );
}
