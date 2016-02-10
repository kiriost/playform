use cgmath::Point;
use std::sync::Mutex;
use stopwatch;
use time;

use common::protocol;
use common::surroundings_loader;
use common::surroundings_loader::LoadType;

use block_position;
use client;
use edge;
use load_terrain;
use load_terrain::lod_index;
use server_update::apply_server_update;
use view_update;
use voxel;

const MAX_OUTSTANDING_TERRAIN_REQUESTS: u32 = 1;

pub fn update_thread<RecvServer, RecvVoxelUpdates, UpdateView0, UpdateView1, UpdateServer, EnqueueBlockUpdates>(
  quit: &Mutex<bool>,
  client: &client::T,
  recv_server: &mut RecvServer,
  recv_voxel_updates: &mut RecvVoxelUpdates,
  update_view0: &mut UpdateView0,
  update_view1: &mut UpdateView1,
  update_server: &mut UpdateServer,
  enqueue_block_updates: &mut EnqueueBlockUpdates,
) where
  RecvServer: FnMut() -> Option<protocol::ServerToClient>,
  RecvVoxelUpdates: FnMut() -> Option<(Vec<(voxel::bounds::T, voxel::T)>, protocol::VoxelReason)>,
  UpdateView0: FnMut(view_update::T),
  UpdateView1: FnMut(view_update::T),
  UpdateServer: FnMut(protocol::ClientToServer),
  EnqueueBlockUpdates: FnMut(Vec<(voxel::bounds::T, voxel::T)>, protocol::VoxelReason),
{
  'update_loop: loop {
    if *quit.lock().unwrap() == true {
      break 'update_loop
    } else {
      stopwatch::time("update_iteration", || {
        stopwatch::time("process_server_updates", || {
          process_server_updates(client, recv_server, update_view0, update_server, enqueue_block_updates);
        });

        stopwatch::time("update_surroundings", || {
          update_surroundings(client, update_view1, update_server);
        });

        stopwatch::time("process_voxel_updates", || {
          process_voxel_updates(client, recv_voxel_updates, update_view1);
        });
      })
    }
  }
}

#[inline(never)]
fn update_surroundings<UpdateView, UpdateServer>(
  client: &client::T,
  update_view: &mut UpdateView,
  update_server: &mut UpdateServer,
) where
  UpdateView: FnMut(view_update::T),
  UpdateServer: FnMut(protocol::ClientToServer),
{
  let start = time::precise_time_ns();
  let mut i = 0;
  let player_position = *client.player_position.lock().unwrap();
  let player_position = block_position::of_world_position(&player_position);
  let mut surroundings_loader = client.surroundings_loader.lock().unwrap();
  let mut updates = surroundings_loader.updates(player_position.as_pnt()) ;
  loop {
    if *client.outstanding_terrain_requests.lock().unwrap() >= MAX_OUTSTANDING_TERRAIN_REQUESTS {
      trace!("update loop breaking");
      break;
    }

    let block_position;
    let load_type;
    match updates.next() {
      None => break,
      Some((b, l)) => {
        block_position = block_position::of_pnt(&b);
        load_type = l;
      },
    }

    debug!("block surroundings");
    let distance =
      surroundings_loader::distance_between(
        player_position.as_pnt(),
        block_position.as_pnt(),
      );
    let new_lod = lod_index(distance);
    let mut requested_voxels = voxel::bounds::set::new();
    for edge in block_position.edges(new_lod).flat_map(|edge| edge.correct_lod(&player_position)) {
      match load_type {
        LoadType::Load => {
          stopwatch::time("update_thread.load_edge", || {
            let already_loaded = client.loaded_edges.lock().unwrap().contains_key(&edge);
            if already_loaded {
              debug!("Not re-loading {:?} at {:?}", block_position, new_lod);
            } else {
              let mut request_voxel = |voxel| {
                requested_voxels.insert(voxel);
              };
              load_or_request_edge(client, &mut request_voxel, update_view, &edge);
            }
          })
        },
        LoadType::Update => {
          stopwatch::time("update_thread.update_block", || {
            // TODO: In this case, if new_lod < current_lod, unload the edge pre-emptively.
          })
        },
        LoadType::Unload => {
          stopwatch::time("update_thread.unload", || {
            // The block removal code is duplicated elsewhere.

            client.loaded_edges
            .lock().unwrap()
              .remove(&edge)
              // If it wasn't loaded, don't unload anything.
              .map(|mesh_fragment| {
                for id in &mesh_fragment.ids {
                  update_view(view_update::RemoveTerrain(*id));
                }
              });
          })
        },
      };
    }

    if !requested_voxels.is_empty() {
      update_server(
        protocol::ClientToServer::RequestVoxels(
          client.id,
          requested_voxels.into_iter().collect(),
        )
      );
      *client.outstanding_terrain_requests.lock().unwrap() += 1;
    }

    if i >= 10 {
      i -= 10;
      if time::precise_time_ns() - start >= 1_000_000 {
        break
      }
    }
    i += 1;
  }
}

fn load_or_request_edge<RequestVoxel, UpdateView>(
  client: &client::T,
  request_voxel: &mut RequestVoxel,
  update_view: &mut UpdateView,
  edge: &edge::T,
) where
  RequestVoxel: FnMut(voxel::bounds::T),
  UpdateView: FnMut(view_update::T),
{
  trace!("Loading edge {:?}", edge);
  match
    load_terrain::load_edge(
      client,
      update_view,
      &edge,
    )
  {
    Ok(()) => {},
    Err(()) => {
      let player_position = block_position::of_world_position(&client.player_position.lock().unwrap());
      let mut voxel_coords = Vec::new();
      voxel_coords.extend(edge.neighbors().iter().cloned());

      let low_corner = edge.low_corner.add_v(&edge.direction.to_vec());
      voxel_coords.push(
        voxel::bounds::T {
          x: low_corner.x,
          y: low_corner.y,
          z: low_corner.z,
          lg_size: edge.lg_size,
        }
      );

      let requests =
        voxel_coords
        .into_iter()
        .flat_map(|voxel| voxel::bounds::correct_lod(&voxel, &player_position));
      for voxel in requests {
        request_voxel(voxel);
      }
    }
  }
}

#[inline(never)]
fn process_voxel_updates<RecvVoxelUpdates, UpdateView>(
  client: &client::T,
  recv_voxel_updates: &mut RecvVoxelUpdates,
  update_view: &mut UpdateView,
) where
  RecvVoxelUpdates: FnMut() -> Option<(Vec<(voxel::bounds::T, voxel::T)>, protocol::VoxelReason)>,
  UpdateView: FnMut(view_update::T),
{
  let start = time::precise_time_ns();
  while let Some((voxel_updates, reason)) = recv_voxel_updates() {
    let mut update_edges = edge::set::new();
    for (bounds, voxel) in voxel_updates {
      trace!("Got voxel at {:?}", bounds);
      load_terrain::load_voxel(
        client,
        &voxel,
        &bounds,
        |edge| { update_edges.insert(edge); },
      );
    }

    for edge in update_edges.into_iter() {
      trace!("voxel-triggered edge loading {:?}", edge);
      let _ =
        load_terrain::load_edge(
          client,
          update_view,
          &edge,
        );
    }

    match reason {
      protocol::VoxelReason::Updated => {},
      protocol::VoxelReason::Requested => {
        *client.outstanding_terrain_requests.lock().unwrap() -= 1;
        debug!("Outstanding terrain requests: {}", *client.outstanding_terrain_requests.lock().unwrap());
      },
    }

    if time::precise_time_ns() - start >= 1_000_000 {
      break
    }
  }
}

#[inline(never)]
fn process_server_updates<RecvServer, UpdateView, UpdateServer, EnqueueBlockUpdates>(
  client: &client::T,
  recv_server: &mut RecvServer,
  update_view: &mut UpdateView,
  update_server: &mut UpdateServer,
  enqueue_block_updates: &mut EnqueueBlockUpdates,
) where
  RecvServer: FnMut() -> Option<protocol::ServerToClient>,
  UpdateView: FnMut(view_update::T),
  UpdateServer: FnMut(protocol::ClientToServer),
  EnqueueBlockUpdates: FnMut(Vec<(voxel::bounds::T, voxel::T)>, protocol::VoxelReason),
{
  let start = time::precise_time_ns();
  let mut i = 0;
  while let Some(up) = recv_server() {
    apply_server_update(
      client,
      update_view,
      update_server,
      enqueue_block_updates,
      up,
    );

    if i > 10 {
      i -= 10;
      if time::precise_time_ns() - start >= 1_000_000 {
        break
      }
    }
    i += 1;
  }
}
