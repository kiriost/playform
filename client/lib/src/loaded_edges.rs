use std;
use cgmath::Vector3;
use voxel_data as voxel;

use edge;

pub struct T<Edge> {
  edges: Vector3<voxel::tree::T<(edge::T, Edge)>>,
}

fn bounds(edge: &edge::T) -> voxel::bounds::T {
  voxel::bounds::new(edge.low_corner.x, edge.low_corner.y, edge.low_corner.z, edge.lg_size)
}

impl<Edge> T<Edge> {
  fn tree(&self, direction: edge::Direction) -> &voxel::tree::T<(edge::T, Edge)> {
    match direction {
      edge::Direction::X => &self.edges.x,
      edge::Direction::Y => &self.edges.y,
      edge::Direction::Z => &self.edges.z,
    }
  }

  fn tree_mut(&mut self, direction: edge::Direction) -> &mut voxel::tree::T<(edge::T, Edge)> {
    match direction {
      edge::Direction::X => &mut self.edges.x,
      edge::Direction::Y => &mut self.edges.y,
      edge::Direction::Z => &mut self.edges.z,
    }
  }

  pub fn insert(&mut self, edge: &edge::T, edge_data: Edge) -> Vec<Edge> {
    let mut removed = Vec::new();
    for collision in self.find_collisions(&edge) {
      removed.push(self.remove(&collision).unwrap());
    }

    let bounds = bounds(&edge);
    self.tree_mut(edge.direction)
      .get_mut_or_create(&bounds)
      .force_branches()
      .data = Some((*edge, edge_data));

    removed
  }

  pub fn remove(&mut self, edge: &edge::T) -> Option<Edge> {
    let mut edges = self.tree_mut(edge.direction);
    let bounds = bounds(&edge);

    match edges.get_mut_pointer(&bounds) {
      Some(&mut voxel::tree::Inner::Branches(ref mut branches)) => {
        let mut r = None;
        std::mem::swap(&mut branches.data, &mut r);
        r.map(|(_, d)| d)
      },
      _ => None,
    }
  }

  pub fn contains_key(&self, edge: &edge::T) -> bool {
    let edges = self.tree(edge.direction);
    let bounds = bounds(&edge);

    edges.get(&bounds).is_some()
  }

  pub fn find_collisions(&self, edge: &edge::T) -> Vec<edge::T> {
    fn all<Edge>(collisions: &mut Vec<edge::T>, branches: &voxel::tree::Inner<(edge::T, Edge)>) {
      if let &voxel::tree::Inner::Branches(ref branches) = branches {
        if let Some((edge, _)) = branches.data {
          collisions.push(edge);
        }

        for branches in branches.as_flat_array() {
          all(collisions, branches);
        }
      }
    }

    let mut collisions = Vec::new();

    let bounds = bounds(edge);
    let edges = self.tree(edge.direction);
    let mut traversal = voxel::tree::traversal::to_voxel(&edges, &bounds);
    let mut edges = &edges.contents;
    loop {
      match traversal.next(edges) {
        voxel::tree::traversal::Step::Last(branches) => {
          all(&mut collisions, branches);
          break;
        },
        voxel::tree::traversal::Step::Step(branches) => {
          if let &voxel::tree::Inner::Branches(ref branches) = branches {
            if let Some((edge, _)) = branches.data {
              collisions.push(edge);
            }
            edges = branches;
          } else {
            break;
          }
        },
      }
    }

    collisions
  }
}

pub fn new<Edge>() -> T<Edge> {
  T {
    edges:
      Vector3::new(
        voxel::tree::new(),
        voxel::tree::new(),
        voxel::tree::new(),
      ),
  }
}
