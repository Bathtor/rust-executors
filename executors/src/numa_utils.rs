// Copyright 2017-2020 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

//! This module contains helpers for executors that are NUMA-ware.
//!
//! In particular the [ProcessingUnitDistance](ProcessingUnitDistance) code,
//! which allows a the description of costs associated with schedulling a function
//! on a different processing unit than it was originally assigned to.

//use hwloc2::{CpuBindFlags, ObjectType, Topology, TopologyObject};
//use std::collections::HashMap;
use core_affinity::CoreId;
use std::iter::FromIterator;

/// A full distance matrix between all processing units on a system
#[derive(Debug, PartialEq)]
pub struct ProcessingUnitDistance {
    distance_matrix: Box<[Box<[i32]>]>,
}
impl ProcessingUnitDistance {
    /// Create a new instance from the underlying distance matrix
    ///
    /// Distance matrices must be square.
    ///
    /// # Panics
    ///
    /// This function will panic if the passed matrix is not square.
    pub fn new(distance_matrix: Box<[Box<[i32]>]>) -> Self {
        // make sure it's a square matrix
        let num_rows = distance_matrix.len();
        for row in distance_matrix.iter() {
            assert_eq!(num_rows, row.len(), "A PU distance matrix must be square!");
        }
        ProcessingUnitDistance { distance_matrix }
    }

    /// A completely empty matrix where all lookups will fail
    ///
    /// This can be used as a placeholder when compiling with feature numa-aware,
    /// but not actually pinning any threads.
    pub fn empty() -> Self {
        ProcessingUnitDistance {
            distance_matrix: Vec::new().into_boxed_slice(),
        }
    }

    /// Materialise the `distance` function into a full matrix of `size` x `size`
    ///
    /// The `distance` function will be invoked with row_index as first argument
    /// and column_index as second argument for each value in `0..size`.
    /// Both indices should be interpreted as if they were `CoreId.id` values.
    ///
    /// The value for `distance(i, i)` should probably be 0, but it is unlikely that any code will rely on this.
    pub fn from_function<F>(size: usize, distance: F) -> Self
    where
        F: Fn(usize, usize) -> i32,
    {
        let mut matrix: Vec<Box<[i32]>> = Vec::with_capacity(size);
        for row_index in 0..size {
            let mut row: Vec<i32> = Vec::with_capacity(size);
            for column_index in 0..size {
                let d = distance(row_index, column_index);
                row.push(d);
            }
            matrix.push(row.into_boxed_slice());
        }
        ProcessingUnitDistance {
            distance_matrix: matrix.into_boxed_slice(),
        }
    }

    /// Get the weighted distance between `pu1` and `pu2` according to this matrix.
    ///
    /// # Panics
    ///
    /// This function will panic if core ids are outside of the matrix bounds.
    pub fn distance(&self, pu1: CoreId, pu2: CoreId) -> i32 {
        self.distance_matrix[pu1.id][pu2.id]
    }

    // Unfinished. Continue this once the hwloc library is more stable
    // pub fn from_topology() -> Result<Self, CalcError> {
    //     let topo = Topology::new().ok_or(CalcError("Topology is unavailable.".to_string()))?;
    //     let mut root_distances: HashMap<String, u32> = HashMap::new();
    //     let pus = collect_root_distances(&mut root_distances, one_norm, &topo)?;
    //     let mut distance_matrix: Vec<Vec<u32>> = Vec::with_capacity(pus.len());
    //     let _ = fill_matrix(&mut distance_matrix, &root_distances, &pus)?;
    //     let partially_boxed_matrix: Vec<Box<[u32]>> = distance_matrix
    //         .into_iter()
    //         .map(|row| row.into_boxed_slice())
    //         .collect();
    //     let boxed_matrix = partially_boxed_matrix.into_boxed_slice();
    //     Ok(PUDistance {
    //         distance_matrix: boxed_matrix,
    //     })
    // }
}

impl FromIterator<Vec<i32>> for ProcessingUnitDistance {
    fn from_iter<I: IntoIterator<Item = Vec<i32>>>(iter: I) -> Self {
        let mut v: Vec<Box<[i32]>> = Vec::new();
        for i in iter {
            v.push(i.into_boxed_slice());
        }
        ProcessingUnitDistance::new(v.into_boxed_slice())
    }
}

impl FromIterator<Box<[i32]>> for ProcessingUnitDistance {
    fn from_iter<I: IntoIterator<Item = Box<[i32]>>>(iter: I) -> Self {
        let mut v: Vec<Box<[i32]>> = Vec::new();
        for i in iter {
            v.push(i);
        }
        ProcessingUnitDistance::new(v.into_boxed_slice())
    }
}

/// Trivial distance function for non-NUMA architectures
///
/// Produces a 0 distance for `i==j` and 1 otherwise.
pub fn equidistance(i: usize, j: usize) -> i32 {
    if i == j {
        0
    } else {
        1
    }
}

/*
* Unfinished. Continue this once the hwloc library is more stable
*/

// type TopologyObjectNorm = fn(&TopologyObject) -> u32;

// fn one_norm(_object: &TopologyObject) -> u32 {
//     1
// }

// fn fill_matrix(
//     distance_matrix: &mut Vec<Vec<u32>>,
//     root_distances: &HashMap<String, u32>,
//     pus: &[&TopologyObject],
// ) -> Result<(), CalcError> {
//     Ok(())
// }

// fn collect_root_distances<'a, 'b>(
//     distance_map: &'a mut HashMap<String, u32>,
//     norm: TopologyObjectNorm,
//     topology: &'b Topology,
// ) -> Result<Box<[&'b TopologyObject]>, CalcError> {
//     let mut collector = DistanceCollector {
//         norm,
//         distance_map,
//         pus: Vec::new(),
//     };
//     println!("About to start at the root...");
//     let root = topology.object_at_root();
//     println!("Root is ok");
//     let root_name = root.name();
//     println!("Root name is ok");
//     println!("Root has name={}", root_name);
//     collector.descend(root, 0)?;
//     Ok(collector.pus.into_boxed_slice())
// }
// struct DistanceCollector<'a, 'b> {
//     norm: TopologyObjectNorm,
//     distance_map: &'a mut HashMap<String, u32>,
//     pus: Vec<&'b TopologyObject>,
// }
// impl<'a, 'b> DistanceCollector<'a, 'b> {
//     fn descend(&mut self, node: &'b TopologyObject, path_cost: u32) -> Result<(), CalcError> {
//         println!("Descending into node={}", node.name());
//         if self.distance_map.insert(node.name(), path_cost).is_some() {
//             return Err(CalcError(format!("Node at {} is not unique!", node.name())));
//         }
//         if node.object_type() == ObjectType::PU {
//             self.pus.push(node);
//             debug_assert_eq!(0, node.arity(), "PUs should have no children!");
//             Ok(())
//         } else {
//             let new_cost = path_cost + (self.norm)(node);
//             for i in 0..node.arity() {
//                 self.descend(node.children()[i as usize], new_cost)?;
//             }
//             Ok(())
//         }
//     }
// }

// #[derive(Debug, Clone)]
// pub(crate) struct CalcError(String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pu_distance_creation() {
        let mat = vec![vec![0, 1], vec![1, 0]];
        let pud: ProcessingUnitDistance = mat.into_iter().collect();
        let pud2 = ProcessingUnitDistance::from_function(2, equidistance);
        assert_eq!(pud, pud2);
        let core1 = CoreId { id: 0 };
        let core2 = CoreId { id: 1 };
        assert_eq!(0, pud.distance(core1, core1));
        assert_eq!(0, pud.distance(core2, core2));
        assert_eq!(1, pud.distance(core1, core2));
        assert_eq!(1, pud.distance(core2, core1));
    }

    #[test]
    fn test_empty_pu_distance() {
        let mat: Vec<Vec<i32>> = Vec::new();
        let pud: ProcessingUnitDistance = mat.into_iter().collect();
        let pud2 = ProcessingUnitDistance::empty();
        assert_eq!(pud, pud2);
    }

    /*
     * Unfinished. Continue this once the hwloc library is more stable
     */
    // #[test]
    // fn test_root_distance_collection() {
    //     let topo = Topology::new().expect("topology");
    //     let mut root_distances: HashMap<String, u32> = HashMap::new();
    //     let pus = collect_root_distances(&mut root_distances, one_norm, &topo).expect("collection");
    //     let pu_names: Vec<String> = pus.iter().map(|pu| pu.name()).collect();
    //     println!("PUs: {:?}", pu_names);
    //     println!("Distances: {:?}", root_distances);
    // }

    // #[test]
    // fn test_core_ids() {
    //     use hwloc2::{CpuBindFlags, Topology};

    //     let core_ids = core_affinity::get_core_ids().unwrap();
    //     println!("cores: {:?}", core_ids);

    //     let topo = Topology::new().unwrap();

    //     for i in 0..topo.depth() {
    //         println!("*** Objects at level {}", i);

    //         for (idx, object) in topo.objects_at_depth(i).iter().enumerate() {
    //             println!("{}: {}", idx, object);
    //         }
    //     }

    //     // Show current binding
    //     //topo.set_cpubind(Bitmap::from(1), CpuBindFlags::CPUBIND_THREAD).expect("thread binding");
    //     core_affinity::set_for_current(core_ids[0]);
    //     println!(
    //         "Current CPU Binding: {:?}",
    //         topo.get_cpubind(CpuBindFlags::CPUBIND_THREAD)
    //     );

    //     // Check if Process Binding for CPUs is supported
    //     println!(
    //         "CPU Binding (current process) supported: {}",
    //         topo.support().cpu().set_current_process()
    //     );
    //     println!(
    //         "CPU Binding (any process) supported: {}",
    //         topo.support().cpu().set_process()
    //     );

    //     // Check if Thread Binding for CPUs is supported
    //     println!(
    //         "CPU Binding (current thread) supported: {}",
    //         topo.support().cpu().set_current_thread()
    //     );
    //     println!(
    //         "CPU Binding (any thread) supported: {}",
    //         topo.support().cpu().set_thread()
    //     );

    //     // Check if Memory Binding is supported
    //     println!(
    //         "Memory Binding supported: {}",
    //         topo.support().memory().set_current_process()
    //     );

    //     // Debug Print all the Support Flags
    //     println!("All Flags:\n{:?}", topo.support());
    // }
}
