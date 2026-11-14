//! Read-only navigation over the authorized V6 projection.
use chronos_client::DependencyWireClientV6;
use chronos_local::LocalClient;
use chronos_model::display_id;
use chronos_protocol::{Reference, Revision, dependencies_v6 as v6};
use std::{path::PathBuf, sync::mpsc};

const LIMIT: usize = 32;

pub(crate) struct Graph {
    directory: PathBuf,
    revision: String,
    anchor: Reference,
    direction: v6::Direction,
    reading: Option<mpsc::Receiver<GraphResult>>,
    result: GraphResult,
    pub(crate) selected: usize,
}

type GraphResult = Result<(Vec<Reference>, bool), &'static str>;

impl Graph {
    pub(crate) fn open(directory: PathBuf, revision: String, anchor: Reference) -> Self {
        let mut graph = Self {
            directory,
            revision,
            anchor,
            direction: v6::Direction::Prerequisites,
            reading: None,
            result: Err("Reading visible edges…"),
            selected: 0,
        };
        graph.load();
        graph
    }

    fn load(&mut self) {
        let (send, receive) = mpsc::sync_channel(1);
        let directory = self.directory.clone();
        let request = v6::Request {
            version: 6,
            command: v6::Command::Neighbors {
                expected: Revision::Finite(self.revision.clone()),
                task: self.anchor.clone(),
                direction: self.direction,
                limit: LIMIT,
            },
        };
        self.reading = Some(receive);
        self.selected = 0;
        self.result = Err("Reading visible edges…");
        std::thread::spawn(move || {
            let result = match LocalClient::new(directory).request_dependencies_v6(request) {
                Ok(response) => match response.outcome {
                    v6::Outcome::VisibleNeighbors {
                        neighbors,
                        truncated,
                        ..
                    } => Ok((neighbors, truncated)),
                    v6::Outcome::Refused {
                        code: v6::Refusal::Unauthorized,
                    } => Err("Dependency read denied"),
                    v6::Outcome::Refused {
                        code: v6::Refusal::Stale,
                    } => Err("Graph is stale. Return to the board and refresh."),
                    v6::Outcome::Refused { .. } => Err("Dependency read refused"),
                },
                Err(_) => Err("Dependency graph unavailable on this service"),
            };
            let _ = send.send(result);
        });
    }

    pub(crate) fn poll(&mut self) -> bool {
        let Some(receiver) = &self.reading else {
            return false;
        };
        self.result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Err("Dependency graph unavailable"),
        };
        self.reading = None;
        true
    }

    pub(crate) fn busy(&self) -> bool {
        self.reading.is_some()
    }

    pub(crate) fn direction(&mut self, direction: v6::Direction) {
        if !self.busy() {
            self.direction = direction;
            self.load();
        }
    }

    pub(crate) fn follow(&mut self) {
        if let Ok((neighbors, _)) = &self.result
            && let Some(next) = neighbors.get(self.selected)
        {
            self.anchor = next.clone();
            self.load();
        }
    }

    pub(crate) fn down(&mut self) {
        if let Ok((neighbors, _)) = &self.result {
            self.selected = (self.selected + 1).min(neighbors.len().saturating_sub(1));
        }
    }

    pub(crate) fn title(&self) -> String {
        let direction = match self.direction {
            v6::Direction::Prerequisites => "Prerequisites",
            v6::Direction::Dependents => "Dependents",
        };
        format!(
            "{direction} of {} · revision {}",
            display_id(&self.anchor.task),
            self.revision
        )
    }

    pub(crate) fn rows(&self) -> (Vec<String>, &'static str) {
        match &self.result {
            Ok((neighbors, true)) => (
                neighbors
                    .iter()
                    .map(|task| display_id(&task.task))
                    .collect(),
                "Additional visible edges omitted",
            ),
            Ok((neighbors, false)) if neighbors.is_empty() => (Vec::new(), "No visible edges"),
            Ok((neighbors, false)) => (
                neighbors
                    .iter()
                    .map(|task| display_id(&task.task))
                    .collect(),
                "Visible edges",
            ),
            Err(reason) => (Vec::new(), reason),
        }
    }
}
