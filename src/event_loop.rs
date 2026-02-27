use std::sync::mpsc;

use crate::command::Command;
use crate::engine::Engine;
use crate::engine_handle::EngineHandle;
use crate::graph::NodeKind;

/// Start the engine on its own thread. Returns an EngineHandle for communication.
pub fn start_engine() -> (EngineHandle, std::thread::JoinHandle<()>) {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let handle = EngineHandle::new(cmd_tx.clone());

    let join = std::thread::spawn(move || {
        run_event_loop(cmd_rx, cmd_tx);
    });

    (handle, join)
}

fn run_event_loop(cmd_rx: mpsc::Receiver<Command>, cmd_tx: mpsc::Sender<Command>) {
    let mut engine = Engine::new();
    engine.async_manager.set_notifier(cmd_tx);

    loop {
        // Block waiting for first command
        let cmd = match cmd_rx.recv() {
            Ok(cmd) => cmd,
            Err(_) => break, // All senders dropped
        };

        // Collect this command and drain all pending commands (batching)
        let mut commands = vec![cmd];
        while let Ok(cmd) = cmd_rx.try_recv() {
            commands.push(cmd);
        }

        let mut has_updates = false;
        let mut should_shutdown = false;

        for cmd in commands {
            match cmd {
                Command::SourceUpdate { id, value } => {
                    // Feed into the engine's internal source channel.
                    // run_cycle() will drain these into the value store.
                    let _ = engine.graph.source_tx.send((id, value));
                    has_updates = true;
                }
                Command::Merge { subgraph, reply } => {
                    let node_ids = subgraph.node_ids.clone();
                    for desc in subgraph.descriptors {
                        engine
                            .graph
                            .register_node(desc.id, desc.kind, desc.deps, desc.node_impl);
                    }
                    for id in &node_ids {
                        if let Some(entry) = engine.graph.entries.get(id) {
                            if entry.kind == NodeKind::Sink {
                                engine.activate_sink(id);
                            }
                        }
                    }
                    let _ = reply.send(node_ids);
                }
                Command::Remove { node_ids } => {
                    for id in node_ids.iter().rev() {
                        engine.deactivate_node(id);
                    }
                }
                Command::AsyncComplete => {
                    // Async result arrived — need to run a cycle to process it
                    has_updates = true;
                }
                Command::Shutdown => {
                    should_shutdown = true;
                }
            }
        }

        // Run a propagation cycle if there were source updates or async completions
        if has_updates {
            engine.run_cycle();
        }

        if should_shutdown {
            break;
        }
    }
}
