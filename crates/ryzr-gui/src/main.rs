use std::{f32::consts::PI, time::Instant};

use bevy::{
    input_focus::InputFocus,
    prelude::*,
    window::{PresentMode, WindowResolution},
};
use ryzr_backend::{Engine, HybridEngine};
use ryzr_core::Circuit;
use ryzr_riscv::{build_cpu, programs};

const MAP_WIDTH: usize = 13;
const MAP_HEIGHT: usize = 13;
const CELL_SIZE: f32 = 1.0;
const CAMERA_HEIGHT: f32 = 0.72;
const VIPER_CLOCK_TICKS_PER_INSTR: u64 = 7;

const MAZE: [&str; MAP_HEIGHT] = [
    "#############",
    "#.....#.....#",
    "#.###.#.###.#",
    "#.#.......#.#",
    "#.#.#####.#.#",
    "#.....#.....#",
    "###.#.#.#.###",
    "#...#...#...#",
    "#.#####.###.#",
    "#.....#.....#",
    "#.###.#.###.#",
    "#.....#.....#",
    "#############",
];

#[derive(Component)]
struct PlayerCamera;

#[derive(Component)]
struct MetricText(MetricKind);

#[derive(Component)]
struct ControlButton(ControlKind);

#[derive(Component)]
struct ControlButtonLabel(ControlKind);

#[derive(Clone, Copy)]
enum MetricKind {
    Status,
    Speed,
    Throughput,
    Frames,
    Checksum,
    Vcb,
}

#[derive(Clone, Copy)]
enum ControlKind {
    RunPause,
    Slower,
    Faster,
    Reset,
}

#[derive(Resource)]
struct PlayerState {
    position: Vec2,
    yaw: f32,
}

struct BenchmarkRunner {
    circuit: Circuit,
    engine: Box<dyn Engine>,
    running: bool,
    ticks_per_update: u64,
    retired: u64,
    last_retired: u64,
    last_sample: Instant,
    ips: f64,
}

impl BenchmarkRunner {
    fn new() -> Self {
        let program = programs::doom_like_benchmark(2047);
        let circuit = build_cpu(&program, 256);
        let engine = Box::new(HybridEngine::new(&circuit));
        Self {
            circuit,
            engine,
            running: true,
            ticks_per_update: 2048,
            retired: 0,
            last_retired: 0,
            last_sample: Instant::now(),
            ips: 0.0,
        }
    }

    fn reset(&mut self) {
        self.engine = Box::new(HybridEngine::new(&self.circuit));
        self.retired = 0;
        self.last_retired = 0;
        self.last_sample = Instant::now();
        self.ips = 0.0;
        self.running = true;
    }

    fn tick_budget(&mut self) {
        if !self.running || self.done() {
            return;
        }
        self.engine.run(self.ticks_per_update);
        self.retired += self.ticks_per_update;

        let elapsed = self.last_sample.elapsed();
        if elapsed.as_millis() >= 250 {
            let delta = self.retired - self.last_retired;
            self.ips = delta as f64 / elapsed.as_secs_f64();
            self.last_retired = self.retired;
            self.last_sample = Instant::now();
        }
    }

    fn slower(&mut self) {
        self.ticks_per_update = (self.ticks_per_update / 2).max(128);
    }

    fn faster(&mut self) {
        self.ticks_per_update = (self.ticks_per_update * 2).min(65_536);
    }

    fn done(&self) -> bool {
        self.reg(17) != 0
    }

    fn reg(&self, register: usize) -> u32 {
        read_word(self.engine.as_ref(), 32 * (register + 1))
    }
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "ryzr editor".to_owned(),
                resolution: WindowResolution::new(1280, 800),
                present_mode: PresentMode::AutoVsync,
                ..default()
            }),
            ..default()
        }))
        .init_resource::<InputFocus>()
        .insert_resource(ClearColor(Color::srgb(0.03, 0.035, 0.04)))
        .insert_resource(PlayerState { position: Vec2::new(1.7, 1.7), yaw: 0.1 })
        .insert_non_send_resource(BenchmarkRunner::new())
        .add_systems(Startup, (setup_scene, setup_ui))
        .add_systems(
            Update,
            (
                control_player,
                update_camera,
                run_benchmark,
                update_metrics,
                update_button_labels,
                handle_buttons,
            ),
        )
        .run();
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let floor_mesh = meshes.add(Plane3d::new(
        Vec3::Y,
        Vec2::new(MAP_WIDTH as f32 * CELL_SIZE, MAP_HEIGHT as f32 * CELL_SIZE),
    ));
    let wall_mesh = meshes.add(Cuboid::new(CELL_SIZE, 1.4, CELL_SIZE));
    let pillar_mesh = meshes.add(Cuboid::new(0.38, 1.7, 0.38));
    let floor_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.11, 0.13, 0.12),
        perceptual_roughness: 0.95,
        ..default()
    });
    let ceiling_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.045, 0.05, 0.055),
        perceptual_roughness: 1.0,
        ..default()
    });
    let wall_material_a = materials.add(StandardMaterial {
        base_color: Color::srgb(0.52, 0.54, 0.48),
        perceptual_roughness: 0.82,
        ..default()
    });
    let wall_material_b = materials.add(StandardMaterial {
        base_color: Color::srgb(0.40, 0.47, 0.54),
        perceptual_roughness: 0.9,
        ..default()
    });
    let marker_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.92, 0.50, 0.25),
        emissive: Color::srgb(0.20, 0.07, 0.02).into(),
        ..default()
    });

    let center = maze_center();
    commands.spawn((
        Mesh3d(floor_mesh.clone()),
        MeshMaterial3d(floor_material),
        Transform::from_xyz(center.x, 0.0, center.y),
    ));
    commands.spawn((
        Mesh3d(floor_mesh),
        MeshMaterial3d(ceiling_material),
        Transform::from_xyz(center.x, 1.42, center.y).with_rotation(Quat::from_rotation_x(PI)),
    ));

    for row in 0..MAP_HEIGHT {
        for col in 0..MAP_WIDTH {
            let world = cell_center(col, row);
            if is_wall_cell(col, row) {
                let material = if (row + col) % 2 == 0 {
                    wall_material_a.clone()
                } else {
                    wall_material_b.clone()
                };
                commands.spawn((
                    Mesh3d(wall_mesh.clone()),
                    MeshMaterial3d(material),
                    Transform::from_xyz(world.x, 0.7, world.y),
                ));
            } else if (row + col) % 9 == 0 {
                commands.spawn((
                    Mesh3d(pillar_mesh.clone()),
                    MeshMaterial3d(marker_material.clone()),
                    Transform::from_xyz(world.x, 0.85, world.y),
                ));
            }
        }
    }

    commands.spawn((
        DirectionalLight { illuminance: 4500.0, shadows_enabled: true, ..default() },
        Transform::from_xyz(-4.0, 7.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        PointLight {
            intensity: 1200.0,
            range: 7.0,
            color: Color::srgb(0.95, 0.72, 0.45),
            ..default()
        },
        Transform::from_xyz(6.2, 1.2, 6.2),
    ));
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(1.7, CAMERA_HEIGHT, 1.7)
            .looking_at(Vec3::new(3.0, CAMERA_HEIGHT, 1.9), Vec3::Y),
        PlayerCamera,
    ));
}

fn setup_ui(mut commands: Commands) {
    commands.spawn(Camera2d);
    commands.spawn((
        Node {
            width: percent(100),
            height: percent(100),
            align_items: AlignItems::Stretch,
            justify_content: JustifyContent::FlexEnd,
            ..default()
        },
        children![(
            Node {
                width: px(318),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(18)),
                row_gap: px(12),
                ..default()
            },
            BackgroundColor(Color::srgba(0.04, 0.045, 0.05, 0.86)),
            children![
                metric(MetricKind::Status, 24.0),
                button_row(),
                metric(MetricKind::Speed, 16.0),
                metric(MetricKind::Throughput, 16.0),
                metric(MetricKind::Frames, 16.0),
                metric(MetricKind::Checksum, 16.0),
                metric(MetricKind::Vcb, 16.0),
            ]
        )],
    ));
}

fn button_row() -> impl Bundle {
    (
        Node { width: percent(100), height: px(38), column_gap: px(8), ..default() },
        children![
            control_button(ControlKind::RunPause, "Pause"),
            control_button(ControlKind::Slower, "-"),
            control_button(ControlKind::Faster, "+"),
            control_button(ControlKind::Reset, "Reset"),
        ],
    )
}

fn control_button(kind: ControlKind, label: &'static str) -> impl Bundle {
    (
        Button,
        ControlButton(kind),
        Node {
            min_width: px(56),
            height: px(34),
            padding: UiRect::horizontal(px(12)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            border: UiRect::all(px(1)),
            ..default()
        },
        BorderColor::all(Color::srgba(0.55, 0.60, 0.62, 0.8)),
        BackgroundColor(Color::srgba(0.12, 0.14, 0.15, 0.94)),
        children![(
            Text::new(label),
            TextFont { font_size: 14.0, ..default() },
            TextColor(Color::srgb(0.90, 0.92, 0.90)),
            ControlButtonLabel(kind),
        )],
    )
}

fn metric(kind: MetricKind, size: f32) -> impl Bundle {
    (
        Text::new(""),
        TextFont { font_size: size, ..default() },
        TextColor(Color::srgb(0.88, 0.91, 0.89)),
        Node { width: percent(100), ..default() },
        MetricText(kind),
    )
}

#[allow(clippy::needless_pass_by_value)]
fn control_player(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut player: ResMut<PlayerState>,
) {
    if keyboard.just_pressed(KeyCode::Space) {
        return;
    }

    let dt = time.delta_secs();
    let turn = 1.8 * dt;
    if keyboard.pressed(KeyCode::ArrowLeft) {
        player.yaw += turn;
    }
    if keyboard.pressed(KeyCode::ArrowRight) {
        player.yaw -= turn;
    }

    let forward = Vec2::new(player.yaw.cos(), player.yaw.sin());
    let right = Vec2::new(-forward.y, forward.x);
    let mut movement = Vec2::ZERO;
    if keyboard.pressed(KeyCode::KeyW) || keyboard.pressed(KeyCode::ArrowUp) {
        movement += forward;
    }
    if keyboard.pressed(KeyCode::KeyS) || keyboard.pressed(KeyCode::ArrowDown) {
        movement -= forward;
    }
    if keyboard.pressed(KeyCode::KeyD) {
        movement += right;
    }
    if keyboard.pressed(KeyCode::KeyA) {
        movement -= right;
    }

    if movement.length_squared() > 0.0 {
        let next = player.position + movement.normalize() * 2.4 * dt;
        if !is_wall_at(next) {
            player.position = next;
        }
    }
}

#[allow(clippy::needless_pass_by_value)]
fn update_camera(player: Res<PlayerState>, mut query: Query<&mut Transform, With<PlayerCamera>>) {
    let mut transform = query.single_mut().expect("one player camera");
    let eye = Vec3::new(player.position.x, CAMERA_HEIGHT, player.position.y);
    let look = eye + Vec3::new(player.yaw.cos(), -0.04, player.yaw.sin());
    *transform = Transform::from_translation(eye).looking_at(look, Vec3::Y);
}

#[allow(clippy::needless_pass_by_value)]
fn run_benchmark(mut benchmark: NonSendMut<BenchmarkRunner>, keyboard: Res<ButtonInput<KeyCode>>) {
    if keyboard.just_pressed(KeyCode::Space) {
        benchmark.running = !benchmark.running;
    }
    if keyboard.just_pressed(KeyCode::BracketLeft) {
        benchmark.slower();
    }
    if keyboard.just_pressed(KeyCode::BracketRight) {
        benchmark.faster();
    }
    if keyboard.just_pressed(KeyCode::KeyR) {
        benchmark.reset();
    }
    benchmark.tick_budget();
}

#[allow(clippy::needless_pass_by_value)]
fn update_metrics(benchmark: NonSend<BenchmarkRunner>, mut query: Query<(&MetricText, &mut Text)>) {
    for (metric, mut text) in &mut query {
        **text = metric_text(metric.0, &benchmark);
    }
}

#[allow(clippy::needless_pass_by_value)]
fn update_button_labels(
    benchmark: NonSend<BenchmarkRunner>,
    mut query: Query<(&ControlButtonLabel, &mut Text)>,
) {
    for (button, mut text) in &mut query {
        **text = button_label(button.0, benchmark.running);
    }
}

fn handle_buttons(
    mut input_focus: ResMut<InputFocus>,
    mut interactions: Query<
        (Entity, &Interaction, &ControlButton, &mut BackgroundColor),
        Changed<Interaction>,
    >,
    mut benchmark: NonSendMut<BenchmarkRunner>,
) {
    for (entity, interaction, button, mut background) in &mut interactions {
        match *interaction {
            Interaction::Pressed => {
                input_focus.set(entity);
                *background = BackgroundColor(Color::srgba(0.22, 0.34, 0.36, 0.98));
                apply_button(button.0, &mut benchmark);
            }
            Interaction::Hovered => {
                input_focus.set(entity);
                *background = BackgroundColor(Color::srgba(0.17, 0.22, 0.23, 0.96));
            }
            Interaction::None => {
                input_focus.clear();
                *background = BackgroundColor(Color::srgba(0.12, 0.14, 0.15, 0.94));
            }
        }
    }
}

fn apply_button(kind: ControlKind, benchmark: &mut BenchmarkRunner) {
    match kind {
        ControlKind::RunPause => benchmark.running = !benchmark.running,
        ControlKind::Slower => benchmark.slower(),
        ControlKind::Faster => benchmark.faster(),
        ControlKind::Reset => benchmark.reset(),
    }
}

fn button_label(kind: ControlKind, running: bool) -> String {
    match kind {
        ControlKind::RunPause => {
            if running {
                "Pause".to_owned()
            } else {
                "Run".to_owned()
            }
        }
        ControlKind::Slower => "-".to_owned(),
        ControlKind::Faster => "+".to_owned(),
        ControlKind::Reset => "Reset".to_owned(),
    }
}

fn metric_text(kind: MetricKind, benchmark: &BenchmarkRunner) -> String {
    let frames = benchmark.reg(11);
    let rays = benchmark.reg(12);
    let hits = benchmark.reg(13);
    match kind {
        MetricKind::Status => {
            if benchmark.done() {
                "ryzr: complete".to_owned()
            } else if benchmark.running {
                "ryzr: running".to_owned()
            } else {
                "ryzr: paused".to_owned()
            }
        }
        MetricKind::Speed => format!("tick budget/update: {}", benchmark.ticks_per_update),
        MetricKind::Throughput => {
            format!("throughput: {:>7.0} instr/s\nretired: {}", benchmark.ips, benchmark.retired)
        }
        MetricKind::Frames => format!("frames: {frames} / 2047\nrays: {rays}\nhits: {hits}"),
        MetricKind::Checksum => format!("checksum: 0x{:08x}", benchmark.reg(10)),
        MetricKind::Vcb => format!(
            "VCB compare: {} ticks/instr\nnominal ticks: {}",
            VIPER_CLOCK_TICKS_PER_INSTR,
            benchmark.retired * VIPER_CLOCK_TICKS_PER_INSTR
        ),
    }
}

fn read_word(engine: &dyn Engine, base: usize) -> u32 {
    (0..32).map(|i| u32::from(engine.output(base + i)) << i).sum()
}

fn is_wall_at(position: Vec2) -> bool {
    if position.x < 0.0 || position.y < 0.0 {
        return true;
    }
    let col = position.x.floor() as usize;
    let row = position.y.floor() as usize;
    is_wall_cell(col, row)
}

fn is_wall_cell(col: usize, row: usize) -> bool {
    if row >= MAP_HEIGHT || col >= MAP_WIDTH {
        return true;
    }
    MAZE[row].as_bytes()[col] == b'#'
}

fn cell_center(col: usize, row: usize) -> Vec2 {
    Vec2::new(col as f32 + 0.5, row as f32 + 0.5)
}

fn maze_center() -> Vec2 {
    Vec2::new(MAP_WIDTH as f32 * 0.5, MAP_HEIGHT as f32 * 0.5)
}
