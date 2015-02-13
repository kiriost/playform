/// Creator of the earth.

use gaia_update::ServerToGaia;
use id_allocator::IdAllocator;
use opencl_context::CL;
use server::EntityId;
use server_update::GaiaToServer;
use std::old_io::timer;
use std::sync::mpsc::{Sender, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::duration::Duration;
use stopwatch::TimerSet;
use terrain::terrain::Terrain;
use terrain::terrain_block::BLOCK_WIDTH;
use terrain::texture_generator::TEXTURE_WIDTH;
use terrain::texture_generator::TerrainTextureGenerator;

pub fn gaia_thread(
  ups_from_server: &Receiver<ServerToGaia>,
  ups_to_server: &Sender<GaiaToServer>,
  id_allocator: &Mutex<IdAllocator<EntityId>>,
  terrain: Arc<Mutex<Terrain>>,
) {
  let timers = TimerSet::new();

  let cl = unsafe {
    CL::new()
  };

  let texture_generators = [
    TerrainTextureGenerator::new(&cl, TEXTURE_WIDTH[0], BLOCK_WIDTH as u32),
    TerrainTextureGenerator::new(&cl, TEXTURE_WIDTH[1], BLOCK_WIDTH as u32),
    TerrainTextureGenerator::new(&cl, TEXTURE_WIDTH[2], BLOCK_WIDTH as u32),
    TerrainTextureGenerator::new(&cl, TEXTURE_WIDTH[3], BLOCK_WIDTH as u32),
  ];

  'gaia_loop:loop {
    'event_loop:loop {
      match ups_from_server.try_recv() {
        Err(TryRecvError::Empty) => break 'event_loop,
        Err(e) => panic!("Error getting world updates: {:?}", e),
        Ok(update) => {
          if !update.apply(
            &timers,
            &cl,
            id_allocator,
            terrain.clone(),
            &texture_generators,
            ups_to_server,
          ) {
            break 'gaia_loop;
          }
        },
      };
    }

    timer::sleep(Duration::milliseconds(0));
  }
}