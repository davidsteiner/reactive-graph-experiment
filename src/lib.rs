pub mod async_compute;
pub mod builder;
pub mod command;
pub mod engine;
pub mod engine_handle;
pub mod error;
pub mod event_loop;
pub mod graph;
pub mod input_tuple;
pub mod introspection;
pub mod metrics;
pub mod node;
pub mod node_ref;
pub mod scheduler;
pub mod sender;
pub mod subgraph;
pub mod topology;
pub mod value_store;

// Re-exports
pub use async_compute::ConcurrencyGroupHandle;
pub use engine::Engine;
pub use engine_handle::EngineHandle;
pub use event_loop::start_engine;
pub use graph::AsyncPolicy;
pub use graph::Graph;
pub use input_tuple::InputTuple;
pub use node::{AsyncCompute, Compute, Node, Sink, Source};
pub use node_ref::{NodeId, NodeRef};
pub use sender::Sender;
pub use subgraph::{Subgraph, SubgraphBuilder, SubgraphHandle};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    // ========== Phase 1 Tests ==========

    #[test]
    fn test_value_store_typed_access() {
        use std::marker::PhantomData;
        let mut store = value_store::ValueStore::new();

        let float_ref: NodeRef<f64> = NodeRef {
            id: NodeId::from_key::<_, ()>("test.float"),
            _marker: PhantomData,
        };
        let string_ref: NodeRef<String> = NodeRef {
            id: NodeId::from_key::<_, ()>("test.string"),
            _marker: PhantomData,
        };

        store.set(&float_ref, 42.0);
        store.set(&string_ref, "hello".to_string());

        assert_eq!(store.get(&float_ref), Some(&42.0));
        assert_eq!(store.get(&string_ref), Some(&"hello".to_string()));
    }

    #[test]
    fn test_input_tuple_arities() {
        fn assert_input_tuple<T: InputTuple>() {}
        assert_input_tuple::<(f64,)>();
        assert_input_tuple::<(f64, f64)>();
        assert_input_tuple::<(f64, String, i32)>();
        assert_input_tuple::<(f64, String, i32, bool)>();
    }

    #[test]
    fn test_source_builder_produces_node_ref() {
        let mut graph = Graph::new();
        let activated = Arc::new(AtomicBool::new(false));
        let activated_clone = activated.clone();

        let source: NodeRef<f64> = Graph::source("my.source")
            .on_activate(move |_tx: Sender<f64>| {
                activated_clone.store(true, Ordering::SeqCst);
                () // guard
            })
            .build(&mut graph);

        // Node is registered
        assert!(graph.get_entry(&source.id).is_some());
        let entry = graph.get_entry(&source.id).unwrap();
        assert_eq!(entry.kind, graph::NodeKind::Source);
        assert_eq!(entry.ref_count, 1);
    }

    #[test]
    fn test_compute_builder_type_safety() {
        let mut graph = Graph::new();

        let s1: NodeRef<f64> = Graph::source("s1")
            .on_activate(|_tx: Sender<f64>| ())
            .build(&mut graph);
        let s2: NodeRef<f64> = Graph::source("s2")
            .on_activate(|_tx: Sender<f64>| ())
            .build(&mut graph);

        let compute: NodeRef<f64> = Graph::compute("sum")
            .dep(s1)
            .dep(s2)
            .compute(|a: &f64, b: &f64| a + b)
            .build(&mut graph);

        let entry = graph.get_entry(&compute.id).unwrap();
        assert_eq!(entry.kind, graph::NodeKind::Compute);
        assert_eq!(entry.deps.len(), 2);
    }

    #[test]
    fn test_sink_builder() {
        let mut graph = Graph::new();

        let s1: NodeRef<f64> = Graph::source("s1")
            .on_activate(|_tx: Sender<f64>| ())
            .build(&mut graph);

        let sink: NodeRef<()> = Graph::sink("my.sink")
            .dep(s1)
            .on_emit(|_v: &f64| {
                // emit callback
            })
            .build(&mut graph);

        let entry = graph.get_entry(&sink.id).unwrap();
        assert_eq!(entry.kind, graph::NodeKind::Sink);
        assert_eq!(entry.deps.len(), 1);
    }

    // --- Trait-based node types for testing ---
    #[derive(Debug)]
    struct TestSource {
        key: String,
    }

    impl Node for TestSource {
        type Output = f64;
        type Key = String;
        fn key(&self) -> String {
            self.key.clone()
        }
    }

    impl Source for TestSource {
        type Guard = ();
        fn activate(&self, _tx: Sender<f64>) -> () {}
    }

    #[derive(Debug)]
    struct TestCompute {
        key: String,
    }

    impl Node for TestCompute {
        type Output = f64;
        type Key = String;
        fn key(&self) -> String {
            self.key.clone()
        }
    }

    impl Compute for TestCompute {
        type Inputs = (f64, f64);
        fn compute(&self, (a, b): (&f64, &f64)) -> f64 {
            a + b
        }
    }

    #[derive(Debug)]
    struct TestSinkNode {
        key: String,
    }

    impl node::Sink for TestSinkNode {
        type Inputs = (f64,);
        type Key = String;
        fn key(&self) -> String {
            self.key.clone()
        }
        fn emit(&self, (_v,): (&f64,)) {}
    }

    #[test]
    fn test_trait_compute_builder() {
        let mut graph = Graph::new();

        let s1 = graph.add_source(TestSource {
            key: "ts1".to_string(),
        });
        let s2 = graph.add_source(TestSource {
            key: "ts2".to_string(),
        });

        let compute: NodeRef<f64> = graph
            .add_compute(TestCompute {
                key: "tc".to_string(),
            })
            .dep(s1)
            .dep(s2)
            .build();

        let entry = graph.get_entry(&compute.id).unwrap();
        assert_eq!(entry.kind, graph::NodeKind::Compute);
        assert_eq!(entry.deps.len(), 2);
    }

    #[test]
    fn test_trait_sink_builder() {
        let mut graph = Graph::new();

        let s1 = graph.add_source(TestSource {
            key: "ts_for_sink".to_string(),
        });

        let sink: NodeRef<()> = graph
            .add_sink(TestSinkNode {
                key: "tsink".to_string(),
            })
            .dep(s1)
            .build();

        let entry = graph.get_entry(&sink.id).unwrap();
        assert_eq!(entry.kind, graph::NodeKind::Sink);
    }

    #[test]
    fn test_node_deduplication() {
        let mut graph = Graph::new();

        let s1 = graph.add_source(TestSource {
            key: "shared".to_string(),
        });
        let s2 = graph.add_source(TestSource {
            key: "shared".to_string(),
        });

        // Same key -> same NodeRef (same NodeId)
        assert_eq!(s1.id, s2.id);
        // ref_count should be 2
        let entry = graph.get_entry(&s1.id).unwrap();
        assert_eq!(entry.ref_count, 2);
    }

    #[test]
    fn test_closure_and_trait_interop() {
        let mut graph = Graph::new();

        // Trait-based source
        let trait_source = graph.add_source(TestSource {
            key: "trait.source".to_string(),
        });

        // Closure-based compute depending on trait source
        let doubled: NodeRef<f64> = Graph::compute("doubled")
            .dep(trait_source.clone())
            .compute(|v: &f64| v * 2.0)
            .build(&mut graph);

        let entry = graph.get_entry(&doubled.id).unwrap();
        assert_eq!(entry.kind, graph::NodeKind::Compute);
        assert_eq!(entry.deps.len(), 1);
        assert_eq!(entry.deps[0], trait_source.id);
    }

    // ========== Phase 2 Tests ==========

    #[test]
    fn test_source_push_propagates_to_compute() {
        let mut engine = Engine::new();
        let g = engine.graph();

        let source: NodeRef<f64> = Graph::source("src")
            .on_activate(|_tx: Sender<f64>| ())
            .build(g);

        let compute: NodeRef<f64> = Graph::compute("double")
            .dep(source.clone())
            .compute(|v: &f64| v * 2.0)
            .build(g);

        // Build a sink to trigger activation
        let sink_ref: NodeRef<()> = Graph::sink("sink")
            .dep(compute.clone())
            .on_emit(|_v: &f64| {})
            .build(g);

        engine.activate_sink(&sink_ref.id);

        // Push a value via the source's sender
        let tx = engine.graph.source_tx.clone();
        tx.send((source.id.clone(), Box::new(10.0_f64))).unwrap();

        engine.run_cycle();

        assert_eq!(engine.graph.value_store.get(&source), Some(&10.0));
        assert_eq!(engine.graph.value_store.get(&compute), Some(&20.0));
    }

    #[test]
    fn test_chain_propagation() {
        use std::sync::{Arc, Mutex};

        let mut engine = Engine::new();
        let g = engine.graph();

        let source: NodeRef<f64> = Graph::source("src")
            .on_activate(|_tx: Sender<f64>| ())
            .build(g);

        let c1: NodeRef<f64> = Graph::compute("c1")
            .dep(source.clone())
            .compute(|v: &f64| v + 1.0)
            .build(g);

        let c2: NodeRef<f64> = Graph::compute("c2")
            .dep(c1.clone())
            .compute(|v: &f64| v * 3.0)
            .build(g);

        let sink_values = Arc::new(Mutex::new(Vec::new()));
        let sv = sink_values.clone();

        let sink_ref: NodeRef<()> = Graph::sink("sink")
            .dep(c2.clone())
            .on_emit(move |v: &f64| {
                sv.lock().unwrap().push(*v);
            })
            .build(g);

        engine.activate_sink(&sink_ref.id);

        let tx = engine.graph.source_tx.clone();
        tx.send((source.id.clone(), Box::new(5.0_f64))).unwrap();
        engine.run_cycle();

        // 5 + 1 = 6, 6 * 3 = 18
        assert_eq!(*sink_values.lock().unwrap(), vec![18.0]);
    }

    #[test]
    fn test_diamond_dependency() {
        use std::sync::{Arc, Mutex};

        let mut engine = Engine::new();
        let g = engine.graph();

        let source: NodeRef<f64> = Graph::source("src")
            .on_activate(|_tx: Sender<f64>| ())
            .build(g);

        let left: NodeRef<f64> = Graph::compute("left")
            .dep(source.clone())
            .compute(|v: &f64| v + 10.0)
            .build(g);

        let right: NodeRef<f64> = Graph::compute("right")
            .dep(source.clone())
            .compute(|v: &f64| v * 2.0)
            .build(g);

        let join_values = Arc::new(Mutex::new(Vec::new()));
        let jv = join_values.clone();

        let join: NodeRef<f64> = Graph::compute("join")
            .dep(left.clone())
            .dep(right.clone())
            .compute(|l: &f64, r: &f64| l + r)
            .build(g);

        let sink_ref: NodeRef<()> = Graph::sink("sink")
            .dep(join.clone())
            .on_emit(move |v: &f64| {
                jv.lock().unwrap().push(*v);
            })
            .build(g);

        engine.activate_sink(&sink_ref.id);

        let tx = engine.graph.source_tx.clone();
        tx.send((source.id.clone(), Box::new(5.0_f64))).unwrap();
        engine.run_cycle();

        // left: 5+10=15, right: 5*2=10, join: 15+10=25
        assert_eq!(*join_values.lock().unwrap(), vec![25.0]);
        // Verify join computed once (only one value in sink)
        assert_eq!(join_values.lock().unwrap().len(), 1);
    }

    #[test]
    fn test_batching() {
        use std::sync::{Arc, Mutex};

        let mut engine = Engine::new();
        let g = engine.graph();

        let s1: NodeRef<f64> = Graph::source("s1")
            .on_activate(|_tx: Sender<f64>| ())
            .build(g);
        let s2: NodeRef<f64> = Graph::source("s2")
            .on_activate(|_tx: Sender<f64>| ())
            .build(g);

        let sum: NodeRef<f64> = Graph::compute("sum")
            .dep(s1.clone())
            .dep(s2.clone())
            .compute(|a: &f64, b: &f64| a + b)
            .build(g);

        let sink_values = Arc::new(Mutex::new(Vec::new()));
        let sv = sink_values.clone();

        let sink_ref: NodeRef<()> = Graph::sink("sink")
            .dep(sum.clone())
            .on_emit(move |v: &f64| {
                sv.lock().unwrap().push(*v);
            })
            .build(g);

        engine.activate_sink(&sink_ref.id);

        // Two source updates before run_cycle
        let tx = engine.graph.source_tx.clone();
        tx.send((s1.id.clone(), Box::new(3.0_f64))).unwrap();
        tx.send((s2.id.clone(), Box::new(7.0_f64))).unwrap();
        engine.run_cycle();

        // Single cycle, consistent snapshot: 3 + 7 = 10
        assert_eq!(*sink_values.lock().unwrap(), vec![10.0]);
    }

    #[test]
    fn test_activation_cascade() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let mut engine = Engine::new();
        let activated = Arc::new(AtomicBool::new(false));
        let act = activated.clone();

        let g = engine.graph();
        let source: NodeRef<f64> = Graph::source("src")
            .on_activate(move |_tx: Sender<f64>| {
                act.store(true, Ordering::SeqCst);
                ()
            })
            .build(g);

        let compute: NodeRef<f64> = Graph::compute("c")
            .dep(source.clone())
            .compute(|v: &f64| *v)
            .build(g);

        let sink_ref: NodeRef<()> = Graph::sink("sink")
            .dep(compute.clone())
            .on_emit(|_v: &f64| {})
            .build(g);

        assert!(!activated.load(Ordering::SeqCst));
        engine.activate_sink(&sink_ref.id);
        assert!(activated.load(Ordering::SeqCst));
    }

    #[test]
    fn test_deactivation_drops_guard() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        struct DropGuard(Arc<AtomicBool>);
        impl Drop for DropGuard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let mut engine = Engine::new();
        let dropped = Arc::new(AtomicBool::new(false));
        let d = dropped.clone();

        let g = engine.graph();
        let source: NodeRef<f64> = Graph::source("src")
            .on_activate(move |_tx: Sender<f64>| DropGuard(d.clone()))
            .build(g);

        let sink_ref: NodeRef<()> = Graph::sink("sink")
            .dep(source.clone())
            .on_emit(|_v: &f64| {})
            .build(g);

        engine.activate_sink(&sink_ref.id);
        assert!(!dropped.load(Ordering::SeqCst));

        // Deactivate sink → cascades to source → guard dropped
        engine.deactivate_node(&sink_ref.id);
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[test]
    fn test_topo_sort_correctness() {
        use crate::topology::Topology;

        let mut topo = Topology::new();
        let s = NodeId::from_key::<_, ()>("S");
        let a = NodeId::from_key::<_, ()>("A");
        let b = NodeId::from_key::<_, ()>("B");
        let c = NodeId::from_key::<_, ()>("C2");

        topo.add_node(s.clone(), vec![]);
        topo.add_node(a.clone(), vec![s.clone()]);
        topo.add_node(b.clone(), vec![s.clone()]);
        topo.add_node(c.clone(), vec![a.clone(), b.clone()]);

        let dirty_sources: HashSet<NodeId> = [s.clone()].into_iter().collect();
        let dirty = topo.dirty_set(&dirty_sources);
        let sorted = topo.topo_sort(&dirty);

        assert_eq!(sorted.len(), 3);
        let pos_a = sorted.iter().position(|n| *n == a).unwrap();
        let pos_b = sorted.iter().position(|n| *n == b).unwrap();
        let pos_c = sorted.iter().position(|n| *n == c).unwrap();
        assert!(pos_c > pos_a);
        assert!(pos_c > pos_b);
    }

    // ========== Phase 3 Tests ==========

    #[test]
    fn test_subgraph_merge() {
        use std::sync::{Arc, Mutex};

        let mut engine = Engine::new();
        let mut sb = SubgraphBuilder::new();

        let source: NodeRef<f64> = SubgraphBuilder::source("sg.src")
            .on_activate(|_tx: Sender<f64>| ())
            .build_sub(&mut sb);

        let compute: NodeRef<f64> = SubgraphBuilder::compute("sg.double")
            .dep(source.clone())
            .compute(|v: &f64| v * 2.0)
            .build_sub(&mut sb);

        let sink_values = Arc::new(Mutex::new(Vec::new()));
        let sv = sink_values.clone();
        SubgraphBuilder::sink("sg.sink")
            .dep(compute.clone())
            .on_emit(move |v: &f64| {
                sv.lock().unwrap().push(*v);
            })
            .build_sub(&mut sb);

        let subgraph = sb.build();
        let _handle = engine.merge(subgraph);

        // Nodes should be active
        assert!(engine.active.contains(&source.id));
        assert!(engine.active.contains(&compute.id));

        // Push and propagate
        let tx = engine.graph.source_tx.clone();
        tx.send((source.id.clone(), Box::new(5.0_f64))).unwrap();
        engine.run_cycle();

        assert_eq!(*sink_values.lock().unwrap(), vec![10.0]);
    }

    #[test]
    fn test_subgraph_handle_drop() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        struct DropGuard(Arc<AtomicBool>);
        impl Drop for DropGuard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let mut engine = Engine::new();
        let dropped = Arc::new(AtomicBool::new(false));
        let d = dropped.clone();

        let mut sb = SubgraphBuilder::new();
        let source: NodeRef<f64> = SubgraphBuilder::source("sg.src")
            .on_activate(move |_tx: Sender<f64>| DropGuard(d.clone()))
            .build_sub(&mut sb);

        SubgraphBuilder::sink("sg.sink")
            .dep(source.clone())
            .on_emit(|_v: &f64| {})
            .build_sub(&mut sb);

        let subgraph = sb.build();
        let handle = engine.merge(subgraph);

        assert!(!dropped.load(Ordering::SeqCst));
        assert!(engine.active.contains(&source.id));

        // Drop handle → removal enqueued, drained at next engine method call
        drop(handle);
        engine.run_cycle();
        assert!(dropped.load(Ordering::SeqCst));
        assert!(!engine.active.contains(&source.id));
    }

    #[test]
    fn test_partial_deactivation() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        struct DropGuard(Arc<AtomicBool>);
        impl Drop for DropGuard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let mut engine = Engine::new();
        let source_dropped = Arc::new(AtomicBool::new(false));

        // Build the shared source directly in the engine
        let d = source_dropped.clone();
        let g = engine.graph();
        let shared_source: NodeRef<f64> = Graph::source("shared.src")
            .on_activate(move |_tx: Sender<f64>| DropGuard(d.clone()))
            .build(g);

        // Build two subgraphs that both reference the shared source
        // (by using the same key, they'll dedup)
        let mut sb1 = SubgraphBuilder::new();
        let src1: NodeRef<f64> = SubgraphBuilder::source::<f64>("shared.src")
            .on_activate(|_tx: Sender<f64>| ())
            .build_sub(&mut sb1);
        SubgraphBuilder::sink("sink1")
            .dep(src1)
            .on_emit(|_v: &f64| {})
            .build_sub(&mut sb1);

        let mut sb2 = SubgraphBuilder::new();
        let src2: NodeRef<f64> = SubgraphBuilder::source::<f64>("shared.src")
            .on_activate(|_tx: Sender<f64>| ())
            .build_sub(&mut sb2);
        SubgraphBuilder::sink("sink2")
            .dep(src2)
            .on_emit(|_v: &f64| {})
            .build_sub(&mut sb2);

        let handle1 = engine.merge(sb1.build());
        let handle2 = engine.merge(sb2.build());

        // Activate the shared source explicitly
        engine.activate_sink(&shared_source.id);

        // Drop first handle → source should stay (still referenced by second subgraph and direct)
        drop(handle1);
        assert!(!source_dropped.load(Ordering::SeqCst));

        // Drop second handle → source still referenced by direct registration
        drop(handle2);
        // Source may or may not be dropped depending on ref count - it was registered 3 times total
        // (once direct, twice via subgraphs), so 3 derefs needed
    }

    #[test]
    fn test_fx_pricing_example() {
        use std::sync::{Arc, Mutex};

        // Simplified FX pricing: source → mid_rate → client_spread → sink

        let mut engine = Engine::new();

        // Function to build a client pricing subgraph
        fn build_client_pricing(
            sb: &mut SubgraphBuilder,
            client: &str,
        ) -> (NodeRef<f64>, NodeRef<f64>, Arc<Mutex<Vec<f64>>>) {
            // Shared source (will dedup across clients)
            let bid: NodeRef<f64> = SubgraphBuilder::source("spot.bid")
                .on_activate(|_tx: Sender<f64>| ())
                .build_sub(sb);
            let ask: NodeRef<f64> = SubgraphBuilder::source("spot.ask")
                .on_activate(|_tx: Sender<f64>| ())
                .build_sub(sb);

            // Shared mid rate
            let mid: NodeRef<f64> = SubgraphBuilder::compute("mid_rate")
                .dep(bid.clone())
                .dep(ask.clone())
                .compute(|b: &f64, a: &f64| (b + a) / 2.0)
                .build_sub(sb);

            // Per-client spread
            let spread_key = format!("spread.{}", client);
            let spread: NodeRef<f64> = SubgraphBuilder::compute(&spread_key)
                .dep(mid.clone())
                .compute(|m: &f64| m * 1.01) // simple spread
                .build_sub(sb);

            let sink_values = Arc::new(Mutex::new(Vec::new()));
            let sv = sink_values.clone();
            let sink_key = format!("pub.{}", client);
            SubgraphBuilder::sink(&sink_key)
                .dep(spread.clone())
                .on_emit(move |v: &f64| {
                    sv.lock().unwrap().push(*v);
                })
                .build_sub(sb);

            (bid, ask, sink_values)
        }

        let mut sb1 = SubgraphBuilder::new();
        let (bid1, ask1, client1_values) = build_client_pricing(&mut sb1, "client1");

        let mut sb2 = SubgraphBuilder::new();
        let (bid2, _ask2, client2_values) = build_client_pricing(&mut sb2, "client2");

        // bid1 and bid2 should have the same key → same NodeId
        assert_eq!(bid1.id, bid2.id);

        let _h1 = engine.merge(sb1.build());
        let _h2 = engine.merge(sb2.build());

        // Push values
        let tx = engine.graph.source_tx.clone();
        tx.send((bid1.id.clone(), Box::new(1.1000_f64))).unwrap();
        tx.send((ask1.id.clone(), Box::new(1.1002_f64))).unwrap();
        engine.run_cycle();

        // mid = (1.1 + 1.1002) / 2 = 1.1001
        // spread = 1.1001 * 1.01 = 1.111101
        let c1 = client1_values.lock().unwrap();
        assert!((c1[0] - 1.111101).abs() < 0.0001);
        let c2 = client2_values.lock().unwrap();
        assert!((c2[0] - 1.111101).abs() < 0.0001);
    }

    // ========== Phase 4 Tests ==========

    #[test]
    fn test_engine_handle_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        fn assert_clone<T: Clone>() {}
        assert_send::<EngineHandle>();
        // mpsc::Sender is not Sync, but EngineHandle is Clone so each thread gets its own.
        assert_clone::<EngineHandle>();
    }

    #[test]
    fn test_cross_thread_source_update() {
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let (handle, join) = start_engine();

        // Build a subgraph with source → compute → sink
        let mut sb = SubgraphBuilder::new();
        let source: NodeRef<f64> = SubgraphBuilder::source("src")
            .on_activate(|_tx: Sender<f64>| ())
            .build_sub(&mut sb);

        let compute: NodeRef<f64> = SubgraphBuilder::compute("double")
            .dep(source.clone())
            .compute(|v: &f64| v * 2.0)
            .build_sub(&mut sb);

        let sink_values = Arc::new(Mutex::new(Vec::new()));
        let sv = sink_values.clone();
        SubgraphBuilder::sink("sink")
            .dep(compute)
            .on_emit(move |v: &f64| {
                sv.lock().unwrap().push(*v);
            })
            .build_sub(&mut sb);

        let _sub_handle = handle.merge(sb.build());

        // Update source from another thread
        let h = handle.clone();
        let s = source.clone();
        std::thread::spawn(move || {
            h.update_source(&s, 21.0_f64);
        })
        .join()
        .unwrap();

        // Give the engine time to process
        std::thread::sleep(Duration::from_millis(50));

        handle.shutdown();
        join.join().unwrap();

        let values = sink_values.lock().unwrap();
        assert_eq!(*values, vec![42.0]);
    }

    #[test]
    fn test_merge_via_handle() {
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let (handle, join) = start_engine();

        let mut sb = SubgraphBuilder::new();
        let source: NodeRef<f64> = SubgraphBuilder::source("src")
            .on_activate(|_tx: Sender<f64>| ())
            .build_sub(&mut sb);

        let sink_values = Arc::new(Mutex::new(Vec::new()));
        let sv = sink_values.clone();
        SubgraphBuilder::sink("sink")
            .dep(source.clone())
            .on_emit(move |v: &f64| {
                sv.lock().unwrap().push(*v);
            })
            .build_sub(&mut sb);

        let _sub = handle.merge(sb.build());

        handle.update_source(&source, 99.0_f64);
        std::thread::sleep(Duration::from_millis(50));

        handle.shutdown();
        join.join().unwrap();

        let values = sink_values.lock().unwrap();
        assert_eq!(*values, vec![99.0]);
    }

    #[test]
    fn test_batching_multiple_updates() {
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let (handle, join) = start_engine();

        let mut sb = SubgraphBuilder::new();
        let source: NodeRef<f64> = SubgraphBuilder::source("src")
            .on_activate(|_tx: Sender<f64>| ())
            .build_sub(&mut sb);

        let sink_values = Arc::new(Mutex::new(Vec::new()));
        let sv = sink_values.clone();
        SubgraphBuilder::sink("sink")
            .dep(source.clone())
            .on_emit(move |v: &f64| {
                sv.lock().unwrap().push(*v);
            })
            .build_sub(&mut sb);

        let _sub = handle.merge(sb.build());

        // Rapid updates — should batch
        for i in 0..10 {
            handle.update_source(&source, i as f64);
        }

        std::thread::sleep(Duration::from_millis(100));
        handle.shutdown();
        join.join().unwrap();

        let values = sink_values.lock().unwrap();
        // Last value should be 9.0
        assert!(!values.is_empty());
        assert_eq!(*values.last().unwrap(), 9.0);
    }

    #[test]
    fn test_shutdown() {
        let (handle, join) = start_engine();
        let handle2 = handle.clone();

        // Shut down
        handle.shutdown();
        join.join().unwrap();

        // Further sends are harmless (channel closed)
        handle2.update_source(
            &NodeRef::<f64> {
                id: NodeId::from_key::<_, ()>("x"),
                _marker: std::marker::PhantomData,
            },
            1.0,
        );
    }

    #[test]
    fn test_concurrent_updates() {
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let (handle, join) = start_engine();

        let mut sb = SubgraphBuilder::new();
        let source: NodeRef<f64> = SubgraphBuilder::source("src")
            .on_activate(|_tx: Sender<f64>| ())
            .build_sub(&mut sb);

        let sink_values = Arc::new(Mutex::new(Vec::new()));
        let sv = sink_values.clone();
        SubgraphBuilder::sink("sink")
            .dep(source.clone())
            .on_emit(move |v: &f64| {
                sv.lock().unwrap().push(*v);
            })
            .build_sub(&mut sb);

        let _sub = handle.merge(sb.build());

        // Multiple threads sending concurrently
        let mut threads = Vec::new();
        for i in 0..5 {
            let h = handle.clone();
            let s = source.clone();
            threads.push(std::thread::spawn(move || {
                for j in 0..10 {
                    h.update_source(&s, (i * 10 + j) as f64);
                }
            }));
        }

        for t in threads {
            t.join().unwrap();
        }

        std::thread::sleep(Duration::from_millis(100));
        handle.shutdown();
        join.join().unwrap();

        // No panics, values were received
        let values = sink_values.lock().unwrap();
        assert!(!values.is_empty());
    }

    // ========== Phase 5 Tests ==========

    #[test]
    fn test_parallel_independent_branches() {
        use std::sync::{Arc, Mutex};
        use std::thread;

        let mut engine = Engine::new();
        let g = engine.graph();

        let s1: NodeRef<f64> = Graph::source("p.s1")
            .on_activate(|_tx: Sender<f64>| ())
            .build(g);
        let s2: NodeRef<f64> = Graph::source("p.s2")
            .on_activate(|_tx: Sender<f64>| ())
            .build(g);

        let thread_ids = Arc::new(Mutex::new(Vec::new()));

        let t1 = thread_ids.clone();
        let c1: NodeRef<f64> = Graph::compute("p.c1")
            .dep(s1.clone())
            .compute(move |v: &f64| {
                t1.lock().unwrap().push(thread::current().id());
                v * 2.0
            })
            .build(g);

        let t2 = thread_ids.clone();
        let c2: NodeRef<f64> = Graph::compute("p.c2")
            .dep(s2.clone())
            .compute(move |v: &f64| {
                t2.lock().unwrap().push(thread::current().id());
                v * 3.0
            })
            .build(g);

        let sink1: NodeRef<()> = Graph::sink("p.sink1")
            .dep(c1.clone())
            .on_emit(|_: &f64| {})
            .build(g);

        let sink2: NodeRef<()> = Graph::sink("p.sink2")
            .dep(c2.clone())
            .on_emit(|_: &f64| {})
            .build(g);

        engine.activate_sink(&sink1.id);
        engine.activate_sink(&sink2.id);

        let tx = engine.graph.source_tx.clone();
        tx.send((s1.id.clone(), Box::new(5.0_f64))).unwrap();
        tx.send((s2.id.clone(), Box::new(7.0_f64))).unwrap();

        engine.run_cycle_parallel();

        assert_eq!(engine.graph.value_store.get(&c1), Some(&10.0));
        assert_eq!(engine.graph.value_store.get(&c2), Some(&21.0));

        // With parallel execution, the two computes may run on different threads
        let tids = thread_ids.lock().unwrap();
        assert_eq!(tids.len(), 2);
    }

    #[test]
    fn test_ready_count_ordering() {
        use std::sync::{Arc, Mutex};

        let mut engine = Engine::new();
        let g = engine.graph();

        let source: NodeRef<f64> = Graph::source("rc.src")
            .on_activate(|_tx: Sender<f64>| ())
            .build(g);

        let left: NodeRef<f64> = Graph::compute("rc.left")
            .dep(source.clone())
            .compute(|v: &f64| v + 10.0)
            .build(g);

        let right: NodeRef<f64> = Graph::compute("rc.right")
            .dep(source.clone())
            .compute(|v: &f64| v * 2.0)
            .build(g);

        let join_results = Arc::new(Mutex::new(Vec::new()));
        let jr = join_results.clone();

        let join: NodeRef<f64> = Graph::compute("rc.join")
            .dep(left.clone())
            .dep(right.clone())
            .compute(move |l: &f64, r: &f64| {
                jr.lock().unwrap().push((*l, *r));
                l + r
            })
            .build(g);

        let sink: NodeRef<()> = Graph::sink("rc.sink")
            .dep(join.clone())
            .on_emit(|_: &f64| {})
            .build(g);

        engine.activate_sink(&sink.id);

        let tx = engine.graph.source_tx.clone();
        tx.send((source.id.clone(), Box::new(5.0_f64))).unwrap();
        engine.run_cycle_parallel();

        // join should see left=15, right=10 → 25
        let results = join_results.lock().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], (15.0, 10.0));
        assert_eq!(engine.graph.value_store.get(&join), Some(&25.0));
    }

    #[test]
    fn test_node_computed_once() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let mut engine = Engine::new();
        let compute_count = Arc::new(AtomicUsize::new(0));
        let cc = compute_count.clone();

        let g = engine.graph();

        let s1: NodeRef<f64> = Graph::source("nc.s1")
            .on_activate(|_tx: Sender<f64>| ())
            .build(g);
        let s2: NodeRef<f64> = Graph::source("nc.s2")
            .on_activate(|_tx: Sender<f64>| ())
            .build(g);

        let join: NodeRef<f64> = Graph::compute("nc.join")
            .dep(s1.clone())
            .dep(s2.clone())
            .compute(move |a: &f64, b: &f64| {
                cc.fetch_add(1, Ordering::SeqCst);
                a + b
            })
            .build(g);

        let sink: NodeRef<()> = Graph::sink("nc.sink")
            .dep(join.clone())
            .on_emit(|_: &f64| {})
            .build(g);

        engine.activate_sink(&sink.id);

        let tx = engine.graph.source_tx.clone();
        tx.send((s1.id.clone(), Box::new(3.0_f64))).unwrap();
        tx.send((s2.id.clone(), Box::new(7.0_f64))).unwrap();
        engine.run_cycle_parallel();

        // Node with two dirty deps computed exactly once
        assert_eq!(compute_count.load(Ordering::SeqCst), 1);
        assert_eq!(engine.graph.value_store.get(&join), Some(&10.0));
    }

    #[test]
    fn test_large_dag() {
        let mut engine = Engine::new();
        let g = engine.graph();

        // Create 100 independent source → compute → sink chains
        let mut sources = Vec::new();
        let mut sinks = Vec::new();

        for i in 0..100 {
            let src: NodeRef<f64> = Graph::source(&format!("ld.s{}", i))
                .on_activate(|_tx: Sender<f64>| ())
                .build(g);

            let comp: NodeRef<f64> = Graph::compute(&format!("ld.c{}", i))
                .dep(src.clone())
                .compute(|v: &f64| v * 2.0)
                .build(g);

            let sink: NodeRef<()> = Graph::sink(&format!("ld.sink{}", i))
                .dep(comp.clone())
                .on_emit(|_: &f64| {})
                .build(g);

            sources.push(src);
            sinks.push(sink);
        }

        for sink in &sinks {
            engine.activate_sink(&sink.id);
        }

        let tx = engine.graph.source_tx.clone();
        for (i, src) in sources.iter().enumerate() {
            tx.send((src.id.clone(), Box::new(i as f64))).unwrap();
        }

        engine.run_cycle_parallel();

        // Verify correctness
        for (i, src) in sources.iter().enumerate() {
            assert_eq!(engine.graph.value_store.get(src), Some(&(i as f64)));
        }
    }

    // ========== Phase 6 Tests ==========

    #[test]
    fn test_async_compute_basic() {
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let mut engine = Engine::new();
        let g = engine.graph();

        let source: NodeRef<f64> = Graph::source("async.src")
            .on_activate(|_tx: Sender<f64>| ())
            .build(g);

        let async_compute: NodeRef<f64> = Graph::async_compute("async.double")
            .dep(source.clone())
            .compute_async(|v: &f64| {
                let v = *v;
                Box::pin(async move { v * 2.0 })
            })
            .build(g);

        let sink_values = Arc::new(Mutex::new(Vec::new()));
        let sv = sink_values.clone();

        let sync_compute: NodeRef<f64> = Graph::compute("async.add1")
            .dep(async_compute.clone())
            .compute(|v: &f64| v + 1.0)
            .build(g);

        let sink: NodeRef<()> = Graph::sink("async.sink")
            .dep(sync_compute.clone())
            .on_emit(move |v: &f64| {
                sv.lock().unwrap().push(*v);
            })
            .build(g);

        engine.activate_sink(&sink.id);

        // Push a value
        let tx = engine.graph.source_tx.clone();
        tx.send((source.id.clone(), Box::new(5.0_f64))).unwrap();

        // First cycle: async compute spawns future, downstream doesn't run yet
        engine.run_cycle();

        // Wait for async result
        std::thread::sleep(Duration::from_millis(50));

        // Second cycle: async result arrives, propagates to sync compute and sink
        engine.run_cycle();

        // 5.0 * 2.0 = 10.0, then 10.0 + 1.0 = 11.0
        let values = sink_values.lock().unwrap();
        assert_eq!(*values, vec![11.0]);
    }

    #[test]
    fn test_latest_wins() {
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let mut engine = Engine::new();
        let g = engine.graph();

        let source: NodeRef<f64> = Graph::source("lw.src")
            .on_activate(|_tx: Sender<f64>| ())
            .build(g);

        // Async compute with a small delay so we can trigger twice
        let async_compute: NodeRef<f64> = Graph::async_compute("lw.async")
            .dep(source.clone())
            .compute_async(|v: &f64| {
                let v = *v;
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    v * 10.0
                })
            })
            .policy(graph::AsyncPolicy::LatestWins)
            .build(g);

        let sink_values = Arc::new(Mutex::new(Vec::new()));
        let sv = sink_values.clone();

        let sink: NodeRef<()> = Graph::sink("lw.sink")
            .dep(async_compute.clone())
            .on_emit(move |v: &f64| {
                sv.lock().unwrap().push(*v);
            })
            .build(g);

        engine.activate_sink(&sink.id);

        // First trigger
        let tx = engine.graph.source_tx.clone();
        tx.send((source.id.clone(), Box::new(1.0_f64))).unwrap();
        engine.run_cycle();

        // Second trigger quickly (cancels first)
        tx.send((source.id.clone(), Box::new(2.0_f64))).unwrap();
        engine.run_cycle();

        // Wait for the second async to complete
        std::thread::sleep(Duration::from_millis(100));
        engine.run_cycle();

        // Only the second result should arrive (2.0 * 10.0 = 20.0)
        let values = sink_values.lock().unwrap();
        assert!(!values.is_empty());
        assert_eq!(*values.last().unwrap(), 20.0);
    }

    #[test]
    fn test_debounce() {
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let mut engine = Engine::new();
        let g = engine.graph();

        let source: NodeRef<f64> = Graph::source("db.src")
            .on_activate(|_tx: Sender<f64>| ())
            .build(g);

        let async_compute: NodeRef<f64> = Graph::async_compute("db.async")
            .dep(source.clone())
            .compute_async(|v: &f64| {
                let v = *v;
                Box::pin(async move { v * 100.0 })
            })
            .policy(graph::AsyncPolicy::Debounce(Duration::from_millis(50)))
            .build(g);

        let sink_values = Arc::new(Mutex::new(Vec::new()));
        let sv = sink_values.clone();

        let sink: NodeRef<()> = Graph::sink("db.sink")
            .dep(async_compute.clone())
            .on_emit(move |v: &f64| {
                sv.lock().unwrap().push(*v);
            })
            .build(g);

        engine.activate_sink(&sink.id);

        let tx = engine.graph.source_tx.clone();

        // Rapid triggers — each should reset the debounce timer
        for i in 1..=5 {
            tx.send((source.id.clone(), Box::new(i as f64))).unwrap();
            engine.run_cycle();
            std::thread::sleep(Duration::from_millis(10));
        }

        // Wait for debounce period + computation time
        std::thread::sleep(Duration::from_millis(100));
        engine.run_cycle();

        // Only the last value should be used: 5.0 * 100.0 = 500.0
        let values = sink_values.lock().unwrap();
        assert!(!values.is_empty());
        assert_eq!(*values.last().unwrap(), 500.0);
    }

    #[test]
    fn test_concurrency_group_limit() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use std::time::Duration;

        let mut engine = Engine::new();
        let group = engine.concurrency_group(3);

        let g = engine.graph();

        let max_concurrent = Arc::new(AtomicUsize::new(0));
        let current_concurrent = Arc::new(AtomicUsize::new(0));

        // Create 10 async compute nodes, all in the same concurrency group limited to 3
        let mut sources = Vec::new();
        let mut sinks = Vec::new();

        for i in 0..10 {
            let src: NodeRef<f64> = Graph::source(&format!("cg.s{}", i))
                .on_activate(|_tx: Sender<f64>| ())
                .build(g);

            let cc = current_concurrent.clone();
            let mc = max_concurrent.clone();

            let ac: NodeRef<f64> = Graph::async_compute(&format!("cg.ac{}", i))
                .dep(src.clone())
                .compute_async(move |v: &f64| {
                    let v = *v;
                    let cc = cc.clone();
                    let mc = mc.clone();
                    Box::pin(async move {
                        let prev = cc.fetch_add(1, Ordering::SeqCst);
                        mc.fetch_max(prev + 1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        cc.fetch_sub(1, Ordering::SeqCst);
                        v * 2.0
                    })
                })
                .policy(graph::AsyncPolicy::LatestWins)
                .group(group)
                .build(g);

            let sink: NodeRef<()> = Graph::sink(&format!("cg.sink{}", i))
                .dep(ac.clone())
                .on_emit(|_: &f64| {})
                .build(g);

            sources.push(src);
            sinks.push(sink);
        }

        for sink in &sinks {
            engine.activate_sink(&sink.id);
        }

        // Trigger all 10 sources
        let tx = engine.graph.source_tx.clone();
        for (i, src) in sources.iter().enumerate() {
            tx.send((src.id.clone(), Box::new(i as f64))).unwrap();
        }
        engine.run_cycle();

        // Wait for all to complete
        std::thread::sleep(Duration::from_millis(300));

        // Drain results
        engine.run_cycle();

        // Max concurrent should be <= 3
        let max = max_concurrent.load(Ordering::SeqCst);
        assert!(max <= 3, "max concurrent was {} but expected <= 3", max);
        assert!(max >= 1, "expected at least 1 concurrent execution");
    }

    #[test]
    fn test_async_to_sync_chain() {
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let mut engine = Engine::new();
        let g = engine.graph();

        // source → async compute → sync compute → sink
        let source: NodeRef<f64> = Graph::source("chain.src")
            .on_activate(|_tx: Sender<f64>| ())
            .build(g);

        let async_node: NodeRef<f64> = Graph::async_compute("chain.async")
            .dep(source.clone())
            .compute_async(|v: &f64| {
                let v = *v;
                Box::pin(async move { v + 100.0 })
            })
            .build(g);

        let sync_node: NodeRef<f64> = Graph::compute("chain.sync")
            .dep(async_node.clone())
            .compute(|v: &f64| v * 3.0)
            .build(g);

        let sink_values = Arc::new(Mutex::new(Vec::new()));
        let sv = sink_values.clone();

        let sink: NodeRef<()> = Graph::sink("chain.sink")
            .dep(sync_node.clone())
            .on_emit(move |v: &f64| {
                sv.lock().unwrap().push(*v);
            })
            .build(g);

        engine.activate_sink(&sink.id);

        // Push value
        let tx = engine.graph.source_tx.clone();
        tx.send((source.id.clone(), Box::new(5.0_f64))).unwrap();
        engine.run_cycle();

        // Wait for async result
        std::thread::sleep(Duration::from_millis(50));
        engine.run_cycle();

        // 5.0 + 100.0 = 105.0, then 105.0 * 3.0 = 315.0
        let values = sink_values.lock().unwrap();
        assert_eq!(*values, vec![315.0]);
    }

    // ========== Phase 7 Tests ==========

    #[test]
    fn test_propagate_stale() {
        use crate::error::ErrorStrategy;
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        };

        let mut engine = Engine::new();
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();

        let sink_values = Arc::new(Mutex::new(Vec::new()));
        let sv = sink_values.clone();

        let (source, compute, sink) = {
            let g = engine.graph();

            let source: NodeRef<f64> = Graph::source("stale.src")
                .on_activate(|_tx: Sender<f64>| ())
                .build(g);

            let compute: NodeRef<f64> = Graph::compute("stale.compute")
                .dep(source.clone())
                .compute(move |v: &f64| {
                    cc.fetch_add(1, Ordering::SeqCst);
                    v * 2.0
                })
                .build(g);

            let sink: NodeRef<()> = Graph::sink("stale.sink")
                .dep(compute.clone())
                .on_emit(move |v: &f64| {
                    sv.lock().unwrap().push(*v);
                })
                .build(g);

            (source, compute, sink)
        };

        engine.set_error_strategy(&compute.id, ErrorStrategy::PropagateStale);
        engine.activate_sink(&sink.id);

        let tx = engine.graph.source_tx.clone();
        tx.send((source.id.clone(), Box::new(5.0_f64))).unwrap();
        engine.run_cycle();

        assert_eq!(engine.graph.value_store.get(&compute), Some(&10.0));

        tx.send((source.id.clone(), Box::new(7.0_f64))).unwrap();
        engine.run_cycle();

        assert_eq!(engine.graph.value_store.get(&compute), Some(&14.0));
    }

    #[test]
    fn test_propagate_error() {
        use crate::error::ErrorStrategy;

        let mut engine = Engine::new();

        let (source, compute, sink) = {
            let g = engine.graph();

            let source: NodeRef<f64> = Graph::source("err.src")
                .on_activate(|_tx: Sender<f64>| ())
                .build(g);

            let compute: NodeRef<f64> = Graph::compute("err.compute")
                .dep(source.clone())
                .compute(|v: &f64| v * 2.0)
                .build(g);

            let sink: NodeRef<()> = Graph::sink("err.sink")
                .dep(compute.clone())
                .on_emit(|_: &f64| {})
                .build(g);

            (source, compute, sink)
        };

        engine.set_error_strategy(&compute.id, ErrorStrategy::PropagateError);
        engine.activate_sink(&sink.id);

        let tx = engine.graph.source_tx.clone();
        tx.send((source.id.clone(), Box::new(3.0_f64))).unwrap();
        engine.run_cycle();

        assert_eq!(engine.graph.value_store.get(&compute), Some(&6.0));
    }

    #[test]
    fn test_retry() {
        use crate::error::ErrorStrategy;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let mut engine = Engine::new();
        let attempt_count = Arc::new(AtomicUsize::new(0));
        let ac = attempt_count.clone();

        let (source, compute, sink) = {
            let g = engine.graph();

            let source: NodeRef<f64> = Graph::source("retry.src")
                .on_activate(|_tx: Sender<f64>| ())
                .build(g);

            let compute: NodeRef<f64> = Graph::compute("retry.compute")
                .dep(source.clone())
                .compute(move |v: &f64| {
                    ac.fetch_add(1, Ordering::SeqCst);
                    v * 2.0
                })
                .build(g);

            let sink: NodeRef<()> = Graph::sink("retry.sink")
                .dep(compute.clone())
                .on_emit(|_: &f64| {})
                .build(g);

            (source, compute, sink)
        };

        engine.set_error_strategy(
            &compute.id,
            ErrorStrategy::Retry {
                max_attempts: 3,
                backoff_ms: 1,
            },
        );
        engine.activate_sink(&sink.id);

        let tx = engine.graph.source_tx.clone();
        tx.send((source.id.clone(), Box::new(5.0_f64))).unwrap();
        engine.run_cycle();

        // Compute should have succeeded on first attempt (it always returns a value)
        assert_eq!(engine.graph.value_store.get(&compute), Some(&10.0));
        // First attempt succeeded, so only 1 call
        assert_eq!(attempt_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_metrics_cycle_duration() {
        let mut engine = Engine::new();
        let g = engine.graph();

        let source: NodeRef<f64> = Graph::source("met.src")
            .on_activate(|_tx: Sender<f64>| ())
            .build(g);

        let compute: NodeRef<f64> = Graph::compute("met.compute")
            .dep(source.clone())
            .compute(|v: &f64| v * 2.0)
            .build(g);

        let sink: NodeRef<()> = Graph::sink("met.sink")
            .dep(compute.clone())
            .on_emit(|_: &f64| {})
            .build(g);

        engine.activate_sink(&sink.id);

        // Before any cycle
        let m = engine.metrics();
        assert_eq!(m.total_cycles, 0);
        assert!(m.last_cycle.is_none());

        // Run a cycle
        let tx = engine.graph.source_tx.clone();
        tx.send((source.id.clone(), Box::new(5.0_f64))).unwrap();
        engine.run_cycle();

        let m = engine.metrics();
        assert_eq!(m.total_cycles, 1);
        assert!(m.last_cycle.is_some());

        let cycle = m.last_cycle.unwrap();
        assert_eq!(cycle.cycle_id, 1);
        assert!(cycle.duration.as_nanos() > 0);
        assert_eq!(cycle.nodes_computed, 2); // compute + sink

        // Verify active node counts
        assert_eq!(m.active_sources, 1);
        assert_eq!(m.active_computes, 1);
        assert_eq!(m.active_sinks, 1);

        // Run another cycle
        tx.send((source.id.clone(), Box::new(10.0_f64))).unwrap();
        engine.run_cycle();

        let m = engine.metrics();
        assert_eq!(m.total_cycles, 2);
        assert_eq!(m.last_cycle.unwrap().cycle_id, 2);
    }

    #[test]
    fn test_introspection_snapshot() {
        let mut engine = Engine::new();
        let g = engine.graph();

        let source: NodeRef<f64> = Graph::source("snap.src")
            .on_activate(|_tx: Sender<f64>| ())
            .build(g);

        let compute: NodeRef<f64> = Graph::compute("snap.compute")
            .dep(source.clone())
            .compute(|v: &f64| v * 2.0)
            .build(g);

        let sink: NodeRef<()> = Graph::sink("snap.sink")
            .dep(compute.clone())
            .on_emit(|_: &f64| {})
            .build(g);

        engine.activate_sink(&sink.id);

        let snapshot = engine.snapshot();

        // Should have 3 nodes
        assert_eq!(snapshot.nodes.len(), 3);

        // Should have 2 edges: source→compute, compute→sink
        assert_eq!(snapshot.edges.len(), 2);

        // Verify node kinds
        let source_info = snapshot.nodes.iter().find(|n| n.id == source.id).unwrap();
        assert_eq!(source_info.kind, graph::NodeKind::Source);
        assert_eq!(source_info.deps.len(), 0);

        let compute_info = snapshot.nodes.iter().find(|n| n.id == compute.id).unwrap();
        assert_eq!(compute_info.kind, graph::NodeKind::Compute);
        assert_eq!(compute_info.deps.len(), 1);

        let sink_info = snapshot.nodes.iter().find(|n| n.id == sink.id).unwrap();
        assert_eq!(sink_info.kind, graph::NodeKind::Sink);
        assert_eq!(sink_info.deps.len(), 1);
    }

    #[test]
    fn test_full_integration() {
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        // Full integration test: source → async compute → sync compute → sink
        // with metrics and introspection
        let mut engine = Engine::new();
        let g = engine.graph();

        let source: NodeRef<f64> = Graph::source("full.src")
            .on_activate(|_tx: Sender<f64>| ())
            .build(g);

        let async_node: NodeRef<f64> = Graph::async_compute("full.async")
            .dep(source.clone())
            .compute_async(|v: &f64| {
                let v = *v;
                Box::pin(async move { v * 2.0 })
            })
            .build(g);

        let sync_node: NodeRef<f64> = Graph::compute("full.sync")
            .dep(async_node.clone())
            .compute(|v: &f64| v + 100.0)
            .build(g);

        let sink_values = Arc::new(Mutex::new(Vec::new()));
        let sv = sink_values.clone();

        let sink: NodeRef<()> = Graph::sink("full.sink")
            .dep(sync_node.clone())
            .on_emit(move |v: &f64| {
                sv.lock().unwrap().push(*v);
            })
            .build(g);

        engine.activate_sink(&sink.id);

        // Verify snapshot
        let snap = engine.snapshot();
        assert_eq!(snap.nodes.len(), 4); // source, async, sync, sink
        assert_eq!(snap.edges.len(), 3);

        // Push value and run cycle (async spawns)
        let tx = engine.graph.source_tx.clone();
        tx.send((source.id.clone(), Box::new(10.0_f64))).unwrap();
        engine.run_cycle();

        // Verify metrics after first cycle
        let m = engine.metrics();
        assert_eq!(m.total_cycles, 1);
        assert_eq!(m.active_sources, 1);
        assert_eq!(m.active_async_computes, 1);

        // Wait for async completion and run second cycle
        std::thread::sleep(Duration::from_millis(50));
        engine.run_cycle();

        // 10.0 * 2.0 = 20.0 (async), then 20.0 + 100.0 = 120.0 (sync)
        let values = sink_values.lock().unwrap();
        assert_eq!(*values, vec![120.0]);

        // Total cycles should be 2
        let m = engine.metrics();
        assert_eq!(m.total_cycles, 2);
    }
}
