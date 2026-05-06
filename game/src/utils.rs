use bevy::{input::InputSystems, prelude::*, window::PrimaryWindow};

pub fn plugin(app: &mut App) {
    app.init_resource::<CursorPos>()
        .add_systems(PreUpdate, cursor_pos_update.after(InputSystems));
}

#[derive(Reflect, Resource, Default, Debug)]
#[reflect(Resource, Default)]
pub struct CursorPos {
    pub world_pos: Vec2,
    pub window_normalized_pos: Vec2,
}

fn cursor_pos_update(
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), Without<IsDefaultUiCamera>>,
    mut cursor_pos: ResMut<CursorPos>,
) {
    let (camera, camera_transform) = *camera;
    let window = window.into_inner();

    if let Some(cursor_position) = window.cursor_position()
        && let Ok(world_pos) = camera.viewport_to_world_2d(camera_transform, cursor_position)
    {
        cursor_pos.window_normalized_pos = vec2(
            cursor_position.x / window.width(),
            cursor_position.y / window.height(),
        );
        cursor_pos.world_pos = world_pos
    }
}
