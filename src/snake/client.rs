use bevy::app::App;
use bevy::prelude::*;
use bevy::utils::HashMap;
use iyes_loopless::prelude::{IntoConditionalSystem, NextState};

use crate::client::resources::ClientPacketManager;
use crate::common::components::{Direction, Position};
use crate::food::components::Food;
use crate::networking::client_packets::{Ready, SnakeMovement};
use crate::networking::server_packets::{SnakePositions, SnakePositionsPacketBuilder, SpawnSnake, SpawnSnakePacketBuilder, SpawnTail, SpawnTailPacketBuilder, StartNewGameAck, StartNewGameAckPacketBuilder};
use crate::snake::{spawn_snake, spawn_tail};
use crate::snake::components::{SnakeHead, SnakeState};
use crate::snake::resources::{ClientId, NumSnakesToSpawn, SnakeId};
use crate::state::GameState;

pub struct SnakeClientPlugin;

impl Plugin for SnakeClientPlugin {
    
    fn build(&self, app: &mut App) {
        app.insert_resource(SnakeId { id: 0 })
            .add_system(wait_for_ack.run_in_state(GameState::ConnectToServer))
            .add_system(pre_game.run_in_state(GameState::PreGame))
            .add_system(update_snake_positions.run_in_state(GameState::Running).label(SnakeState::Movement))
            .add_system(handle_spawn_tail.run_in_state(GameState::Running).after(SnakeState::Movement))
            .add_system(snake_movement_input.run_in_state(GameState::Running).after(SnakeState::Movement));
    }
}

fn wait_for_ack(mut commands: Commands, mut manager: ResMut<ClientPacketManager>) {
    let ack = manager.manager.received::<StartNewGameAck, StartNewGameAckPacketBuilder>(false).unwrap();
    // TODO: Validate only one ack received
    if let Some(ack) = ack {
        if !ack.is_empty() {
            info!("[client] Got StartNewGameAck from server, with expected number of snakes={}", ack[0].num_snakes);
            commands.insert_resource(NumSnakesToSpawn { num: ack[0].num_snakes as i32 });
            commands.insert_resource(ClientId { id: ack[0].client_id });
            commands.insert_resource(NextState(GameState::PreGame));
        }
    }
}

fn pre_game(mut commands: Commands, mut manager: ResMut<ClientPacketManager>, mut num_snakes: ResMut<NumSnakesToSpawn>, mut snake_id: ResMut<SnakeId>) {
    let snake_spawns = manager.manager.received::<SpawnSnake, SpawnSnakePacketBuilder>(false).unwrap();
    if let Some(snake_spawns) = snake_spawns {
        for spawn in snake_spawns.iter() {
            if spawn.id < snake_id.id {
                continue;  // Skip as we already processed this snake spawn
            } else if spawn.id > snake_id.id {
                panic!("[client] Received snake id={} from server that did not match client's tracked id={}", spawn.id, snake_id.id);
            }
            spawn_snake(&mut commands, spawn.id, Position { x: spawn.position.0, y: spawn.position.1 }, Color::rgb(spawn.sRGB.0, spawn.sRGB.1, spawn.sRGB.2));
            snake_id.id += 1;
            num_snakes.num -= 1;
            if num_snakes.num < 0 {
                panic!("[client] Spawned more snakes than expected!")
            }
        }
        
        if num_snakes.num == 0 {
            manager.send(Ready).unwrap();
        }
    }
}

fn update_snake_positions(mut manager: ResMut<ClientPacketManager>, mut q: Query<(&mut Position, &mut SnakeHead)>, mut tail_positions: Query<&mut Position, (Without<SnakeHead>, Without<Food>)>) {
    let snake_positions = manager.manager.received::<SnakePositions, SnakePositionsPacketBuilder>(false).unwrap();
    if let Some(snake_positions) = snake_positions {
        let mut snakes = HashMap::new();
        for (pos, head) in q.iter_mut() {
            snakes.insert(head.id, (pos, head));
        }
        
        for snake_position in snake_positions.iter() {
            for orientation in snake_position.positions.iter() {
                match snakes.get_mut(&orientation.id) {
                    None => {
                        panic!("[client] Snake with ID {} does not exist!", orientation.id);
                    }
                    Some((pos, head)) => {
                        pos.x = orientation.position.0;
                        pos.y = orientation.position.1;
                        head.input_direction = orientation.input_direction;
                        head.direction = orientation.direction;

                        let server_tail_len = orientation.tail_positions.len();

                        // Only modify the old tail positions, new ones should already be in the right place
                        for (i, entity) in head.tail.iter().enumerate() {
                            if i >= server_tail_len {
                                break;  // If client got SpawnTail packet before server has updated
                            }
                            let mut tail_pos = tail_positions.get_mut(*entity).unwrap();
                            tail_pos.x = orientation.tail_positions[i].0;
                            tail_pos.y = orientation.tail_positions[i].1;
                        }
                    }
                }
            }
        }
    }
}

fn snake_movement_input(keys: Res<Input<KeyCode>>, mut head_positions: Query<&mut SnakeHead>, mut manager: ResMut<ClientPacketManager>, client_id: Res<ClientId>) {
    for mut head in head_positions.iter_mut() {
        if head.id == client_id.id {
            let dir: Direction = if keys.pressed(KeyCode::Left) {
                Direction::Left
            } else if keys.pressed(KeyCode::Down) {
                Direction::Down
            } else if keys.pressed(KeyCode::Up) {
                Direction::Up
            } else if keys.pressed(KeyCode::Right) {
                Direction::Right
            } else {
                head.input_direction
            };
            if dir != head.direction.opposite() && dir != head.input_direction {
                head.input_direction = dir;
                manager.manager.send(SnakeMovement { id: head.id, direction: head.input_direction }).unwrap();
            }
            
            break;
        }
    }
}

fn handle_spawn_tail(mut commands: Commands, mut manager: ResMut<ClientPacketManager>, mut q: Query<(&mut Position, &mut SnakeHead)>) {
    let spawn_tails = manager.manager.received::<SpawnTail, SpawnTailPacketBuilder>(false).unwrap();
    if let Some(spawn_tails) = spawn_tails {
        for st in spawn_tails.iter() {
            let mut snakes = HashMap::new();
            for (pos, head) in q.iter_mut() {
                snakes.insert(head.id, (pos, head));
            }

            match snakes.get_mut(&st.id) {
                None => {
                    panic!("[client] Snake with ID {} does not exist!", st.id);
                }
                Some((_pos, head)) => {
                    let head_color = head.color;
                    head.tail.push(spawn_tail(&mut commands, Position { x: st.position.0, y: st.position.1 }, None, st.id, &head_color));
                    info!("[client] Spawned tail at {}, {} for Snake Id {}", st.position.0, st.position.1, st.id);
                }
            }
        }
    }
}