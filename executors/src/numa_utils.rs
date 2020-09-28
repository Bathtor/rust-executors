use hwloc2::{CpuBindFlags, ObjectType, Topology, TopologyObject};
use std::collections::HashMap;

/// A full distance matrix between all processing units on a system
pub(crate) struct PUDistance {
    distance_matrix: Box<[Box<[u32]>]>,
}
impl PUDistance {
    pub(crate) fn from_topology() -> Result<Self, CalcError> {
        let topo = Topology::new().ok_or(CalcError("Topology is unavailable.".to_string()))?;
        let mut root_distances: HashMap<String, u32> = HashMap::new();
        let pus = collect_root_distances(&mut root_distances, one_norm, &topo)?;
        let mut distance_matrix: Vec<Vec<u32>> = Vec::with_capacity(pus.len());
        let _ = fill_matrix(&mut distance_matrix, &root_distances, &pus)?;
        let partially_boxed_matrix: Vec<Box<[u32]>> = distance_matrix
            .into_iter()
            .map(|row| row.into_boxed_slice())
            .collect();
        let boxed_matrix = partially_boxed_matrix.into_boxed_slice();
        Ok(PUDistance {
            distance_matrix: boxed_matrix,
        })
    }
}

type TopologyObjectNorm = fn(&TopologyObject) -> u32;

fn one_norm(_object: &TopologyObject) -> u32 {
    1
}

fn fill_matrix(
    distance_matrix: &mut Vec<Vec<u32>>,
    root_distances: &HashMap<String, u32>,
    pus: &[&TopologyObject],
) -> Result<(), CalcError> {
    Ok(())
}

fn collect_root_distances<'a, 'b>(
    distance_map: &'a mut HashMap<String, u32>,
    norm: TopologyObjectNorm,
    topology: &'b Topology,
) -> Result<Box<[&'b TopologyObject]>, CalcError> {
    let mut collector = DistanceCollector {
        norm,
        distance_map,
        pus: Vec::new(),
    };
    println!("About to start at the root...");
    let root = topology.object_at_root();
    println!("Root is ok");
    let root_name = root.name();
    println!("Root name is ok");
    println!("Root has name={}", root_name);
    collector.descend(root, 0)?;
    Ok(collector.pus.into_boxed_slice())
}
struct DistanceCollector<'a, 'b> {
    norm: TopologyObjectNorm,
    distance_map: &'a mut HashMap<String, u32>,
    pus: Vec<&'b TopologyObject>,
}
impl<'a, 'b> DistanceCollector<'a, 'b> {
    fn descend(&mut self, node: &'b TopologyObject, path_cost: u32) -> Result<(), CalcError> {
        println!("Descending into node={}", node.name());
        if self.distance_map.insert(node.name(), path_cost).is_some() {
            return Err(CalcError(format!("Node at {} is not unique!", node.name())));
        }
        if node.object_type() == ObjectType::PU {
            self.pus.push(node);
            debug_assert_eq!(0, node.arity(), "PUs should have no children!");
            Ok(())
        } else {
            let new_cost = path_cost + (self.norm)(node);
            for i in 0..node.arity() {
                self.descend(node.children()[i as usize], new_cost)?;
            }
            Ok(())
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CalcError(String);

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn test_root_distance_collection() {
        let topo = Topology::new().expect("topology");
        let mut root_distances: HashMap<String, u32> = HashMap::new();
        let pus = collect_root_distances(&mut root_distances, one_norm, &topo).expect("collection");
        let pu_names: Vec<String> = pus.iter().map(|pu| pu.name()).collect();
        println!("PUs: {:?}", pu_names);
        println!("Distances: {:?}", root_distances);
    }

    #[test]
    fn test_core_ids() {
        use hwloc2::{CpuBindFlags, Topology};

        let core_ids = core_affinity::get_core_ids().unwrap();
        println!("cores: {:?}", core_ids);

        let topo = Topology::new().unwrap();

        for i in 0..topo.depth() {
            println!("*** Objects at level {}", i);

            for (idx, object) in topo.objects_at_depth(i).iter().enumerate() {
                println!("{}: {}", idx, object);
            }
        }

        // Show current binding
        //topo.set_cpubind(Bitmap::from(1), CpuBindFlags::CPUBIND_THREAD).expect("thread binding");
        core_affinity::set_for_current(core_ids[0]);
        println!(
            "Current CPU Binding: {:?}",
            topo.get_cpubind(CpuBindFlags::CPUBIND_THREAD)
        );

        // Check if Process Binding for CPUs is supported
        println!(
            "CPU Binding (current process) supported: {}",
            topo.support().cpu().set_current_process()
        );
        println!(
            "CPU Binding (any process) supported: {}",
            topo.support().cpu().set_process()
        );

        // Check if Thread Binding for CPUs is supported
        println!(
            "CPU Binding (current thread) supported: {}",
            topo.support().cpu().set_current_thread()
        );
        println!(
            "CPU Binding (any thread) supported: {}",
            topo.support().cpu().set_thread()
        );

        // Check if Memory Binding is supported
        println!(
            "Memory Binding supported: {}",
            topo.support().memory().set_current_process()
        );

        // Debug Print all the Support Flags
        println!("All Flags:\n{:?}", topo.support());
    }
}
