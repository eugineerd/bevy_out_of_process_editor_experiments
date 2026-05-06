use bevy::camera_controller::free_camera::FreeCameraPlugin;
use bevy::feathers::controls::{ButtonProps, button};
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, observe};
use jackdaw_sdk::runtime::out_of_process::{EditorBtn, EditorIntegrationPlugin, EditorSync};

#[derive(Component)]
pub struct DebugView;
mod feathers;
mod utils;

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins,
            EditorIntegrationPlugin::default(),
            FreeCameraPlugin,
            utils::plugin,
            feathers::plugin,
        ))
        // .add_plugins((
        //     DefaultPlugins,
        //     jackdaw_remote::JackdawRemotePlugin::default(),
        // ))
        //
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
    // commands.spawn(Camera2d::default());
    commands.spawn((
        Camera3d::default(),
        bevy::camera_controller::free_camera::FreeCamera::default(),
    ));
    commands.spawn((
        EditorSync {},
        button(
            ButtonProps {
                ..Default::default()
            },
            (),
            Spawn((Text::new("Normal"))),
        ),
        EditorBtn {},
        GlobalZIndex(10000000),
        observe(|_activate: On<Activate>| {
            info!("Normal button clicked!");
        }),
    ));
}

fn circle(mut gizmos: Gizmos, cursor_pos: Res<utils::CursorPos>, time: Res<Time>) {
    gizmos.circle_2d(
        cursor_pos.world_pos,
        cursor_pos
            .window_normalized_pos
            .x
            .remap(0.0, 1.0, 0.0, 50.0),
        Color::linear_rgb(1.0, 1.0, 0.5),
    );
}
