use cgmath::Point3;
use num;
use std::collections::hash_map::Entry::{Vacant, Occupied};

use common::surroundings_loader;
use common::voxel;

use block_position;
use client;
use lod;
use terrain_mesh;
use view_update::ClientToView;

#[inline(never)]
fn updated_block_positions(
  voxel: &voxel::bounds::T,
) -> Vec<block_position::T>
{
  let block = block_position::containing_voxel(voxel);

  macro_rules! tweak(($dim:ident) => {{
    let mut new_voxel = voxel.clone();
    new_voxel.$dim += 1;
    if block_position::containing_voxel(&new_voxel) != block {
      1
    } else {
      let mut new_voxel = voxel.clone();
      new_voxel.$dim -= 1;
      if block_position::containing_voxel(&new_voxel) != block {
        -1
      } else {
        0
      }
    }
  }});

  let tweak =
    Point3::new(
      tweak!(x),
      tweak!(y),
      tweak!(z),
    );

  macro_rules! consider(($dim:ident, $block:expr, $next:expr) => {{
    $next($block);
    if tweak.$dim != 0 {
      let mut block = $block;
      block.as_mut_pnt().$dim += tweak.$dim;
      $next(block);
    }
  }});

  let mut blocks = Vec::new();
  consider!(x, block, |block: block_position::T| {
  consider!(y, block, |block: block_position::T| {
  consider!(z, block, |block: block_position::T| {
    blocks.push(block);
  })})});

  blocks
}

pub fn all_voxels_loaded(
  block_voxels_loaded: &block_position::with_lod::map::T<u32>,
  block_position: block_position::T,
  lod: lod::T,
) -> bool {
  let block_voxels_loaded =
    match block_voxels_loaded.get(&(block_position, lod)) {
      None => return false,
      Some(x) => x,
    };

  let edge_samples = terrain_mesh::EDGE_SAMPLES[lod.0 as usize] as u32 + 2;
  let samples = edge_samples * edge_samples * edge_samples;
  assert!(*block_voxels_loaded <= samples, "{:?}", block_position);
  *block_voxels_loaded == samples
}

#[inline(never)]
pub fn load_voxel<UpdateBlock>(
  client: &client::T,
  voxel: voxel::T,
  bounds: voxel::bounds::T,
  mut update_block: UpdateBlock,
) where
  UpdateBlock: FnMut(block_position::T, lod::T),
{
  let player_position =
    block_position::of_world_position(&client.player_position.lock().unwrap().clone());

  let mut voxels = client.voxels.lock().unwrap();
  let mut block_voxels_loaded = client.block_voxels_loaded.lock().unwrap();

  // Has a new voxel been loaded? (in contrast to changing an existing voxel)
  let new_voxel_loaded;
  {
    let branch = voxels.get_mut_or_create(&bounds);
    match branch {
      &mut voxel::tree::Empty => {
        *branch =
          voxel::tree::Branch {
            data: Some(voxel),
            branches: Box::new(voxel::tree::Branches::empty()),
          };
        new_voxel_loaded = true;
      },
      &mut voxel::tree::Branch { ref mut data, .. } => {
        match data {
          &mut None => new_voxel_loaded = true,
          &mut Some(old_voxel) => {
            new_voxel_loaded = false;
            if old_voxel == voxel {
              return
            }
          },
        }
        *data = Some(voxel);
      }
    }
  }

  trace!("voxel bounds {:?}", bounds);

  // The LOD of the blocks that should be updated.
  // This doesn't necessarily match the LOD they're loaded at.
  let mut updated_lod = None;
  for lod in 0..terrain_mesh::LOD_COUNT as u32 {
    let lod = lod::T(lod);

    let lg_size = terrain_mesh::LG_SAMPLE_SIZE[lod.0 as usize];
    if lg_size == bounds.lg_size {
      updated_lod = Some(lod);
      break
    }
  }

  for block_position in updated_block_positions(&bounds).into_iter() {
    trace!("block_position {:?}", block_position);
    if new_voxel_loaded {
      match updated_lod {
        None => {}
        Some(updated_lod) => {
          let block_voxels_loaded =
            block_voxels_loaded.entry((block_position, updated_lod))
            .or_insert_with(|| 0);
          trace!("{:?} gets {:?}", block_position, bounds);
          *block_voxels_loaded += 1;
        },
      }
    }

    let distance = surroundings_loader::distance_between(player_position.as_pnt(), &block_position.as_pnt());

    if distance > client.max_load_distance {
      debug!(
        "Not loading {:?}: too far away from player at {:?}.",
        bounds,
        player_position,
      );
      continue;
    }

    let lod = lod_index(distance);
    let lg_size = terrain_mesh::LG_SAMPLE_SIZE[lod.0 as usize];
    if lg_size != bounds.lg_size {
      debug!(
        "{:?} is not the desired LOD {:?}.",
        bounds,
        lod
      );
      continue;
    }

    if all_voxels_loaded(&block_voxels_loaded, block_position, lod) {
      update_block(block_position, lod);
    }
  }
}

#[inline(never)]
pub fn load_block<UpdateView>(
  client: &client::T,
  update_view: &mut UpdateView,
  block_position: &block_position::T,
  lod: lod::T,
) where
  UpdateView: FnMut(ClientToView),
{
  debug!("generate {:?} at {:?}", block_position, lod);
  let voxels = client.voxels.lock().unwrap();
  let mesh_block = terrain_mesh::generate(&voxels, &block_position, lod, &client.id_allocator);

  let mut updates = Vec::new();

  // TODO: Rc instead of clone.
  match client.loaded_blocks.lock().unwrap().entry(block_position.clone()) {
    Vacant(entry) => {
      entry.insert((mesh_block.clone(), lod));
    },
    Occupied(mut entry) => {
      {
        // The mesh_block removal code is duplicated elsewhere.

        let &(ref prev_block, _) = entry.get();
        for &id in &prev_block.ids {
          updates.push(ClientToView::RemoveTerrain(id));
        }
      }
      entry.insert((mesh_block.clone(), lod));
    },
  };

  if !mesh_block.ids.is_empty() {
    updates.push(ClientToView::AddBlock(block_position.clone(), mesh_block, lod));
  }

  update_view(ClientToView::Atomic(updates));
}

pub fn lod_index(distance: i32) -> lod::T {
  assert!(distance >= 0);
  let mut lod = 0;
  while
    lod < client::LOD_THRESHOLDS.len()
    && client::LOD_THRESHOLDS[lod] < distance
  {
    lod += 1;
  }
  lod::T(num::traits::FromPrimitive::from_usize(lod).unwrap())
}
