use std::sync::Arc;

use xv_core::SourceId;

use crate::DataSource;

#[derive(Default)]
pub struct SourceRegistry {
    sources: Vec<Arc<dyn DataSource>>,
}

impl SourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, source: Arc<dyn DataSource>) {
        self.sources.push(source);
    }

    pub fn get(&self, id: &SourceId) -> Option<Arc<dyn DataSource>> {
        self.sources.iter().find(|s| &s.id() == id).cloned()
    }

    pub fn all(&self) -> Vec<Arc<dyn DataSource>> {
        self.sources.clone()
    }

    /// The first source added.
    pub fn default_source(&self) -> Option<Arc<dyn DataSource>> {
        self.sources.first().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::tests::FakeSource;

    #[test]
    fn registry_default_is_first() {
        let mut r = SourceRegistry::new();
        assert!(r.default_source().is_none());
        r.add(Arc::new(FakeSource::new("a")));
        r.add(Arc::new(FakeSource::new("b")));
        assert_eq!(r.default_source().unwrap().id(), SourceId("a".into()));
        assert_eq!(r.get(&SourceId("b".into())).unwrap().name(), "fake");
        assert!(r.get(&SourceId("c".into())).is_none());
        assert_eq!(r.all().len(), 2);
    }
}
