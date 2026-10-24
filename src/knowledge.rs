//! An editable, sourced graph of claims, separate from frozen model weights.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

use crate::errors::{invalid_data as invalid, invalid_json};
use serde::{Deserialize, Serialize};

const VERSION: u64 = 1;
const MAX_ENTRIES: usize = 10_000;
const MAX_FIELD_CHARS: usize = 500;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Claim {
    pub id: u64,
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub source: String,
    pub supersedes: Option<u64>,
    pub retracted_by: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct KnowledgeMap {
    version: u64,
    entries: Vec<Claim>,
}

impl Default for KnowledgeMap {
    fn default() -> Self {
        Self {
            version: VERSION,
            entries: Vec::new(),
        }
    }
}

impl KnowledgeMap {
    pub fn load(path: &Path) -> io::Result<Self> {
        if !path.exists() {
            return Ok(Self {
                version: VERSION,
                entries: Vec::new(),
            });
        }
        let bytes = fs::read(path)?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(invalid("knowledge map exceeds 16 MiB"));
        }
        let map: Self = serde_json::from_slice(&bytes).map_err(invalid_json)?;
        if map.version != VERSION || map.entries.len() > MAX_ENTRIES {
            return Err(invalid("unsupported or oversized knowledge map"));
        }
        let mut ids = BTreeSet::new();
        let mut previous = 0;
        for entry in &map.entries {
            if entry.id <= previous || !ids.insert(entry.id) {
                return Err(invalid("knowledge map has unordered or duplicate IDs"));
            }
            previous = entry.id;
            for field in [
                &entry.subject,
                &entry.relation,
                &entry.object,
                &entry.source,
            ] {
                check_field(field)?;
            }
        }
        Ok(map)
    }

    pub fn active_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.retracted_by.is_none())
            .count()
    }

    pub fn entries(&self) -> &[Claim] {
        &self.entries
    }

    pub fn add(
        &mut self,
        subject: &str,
        relation: &str,
        object: &str,
        source: &str,
    ) -> io::Result<u64> {
        self.insert(subject, relation, object, source, None)
    }

    pub fn revise(
        &mut self,
        id: u64,
        subject: &str,
        relation: &str,
        object: &str,
        source: &str,
    ) -> io::Result<u64> {
        check_field(source)?;
        // Validate the replacement before changing the old claim.
        for field in [subject, relation, object] {
            check_field(field)?;
        }
        let index = self.active_index(id)?;
        if self.entries.len() >= MAX_ENTRIES {
            return Err(invalid("knowledge map is full"));
        }
        self.entries[index].retracted_by = Some(source.trim().into());
        self.insert(subject, relation, object, source, Some(id))
    }

    pub fn retract(&mut self, id: u64, source: &str) -> io::Result<()> {
        check_field(source)?;
        let index = self.active_index(id)?;
        self.entries[index].retracted_by = Some(source.trim().into());
        Ok(())
    }

    pub fn relevant<'a>(&'a self, query: &str, limit: usize) -> Vec<&'a Claim> {
        let query_words = words(query);
        if query_words.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut scored: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.retracted_by.is_none())
            .filter_map(|entry| {
                let subject = words(&entry.subject);
                let relation = words(&entry.relation);
                let object = words(&entry.object);
                let subject_match = lexical_overlap(&query_words, &subject);
                let relation_match = lexical_overlap(&query_words, &relation);
                let object_match = lexical_overlap(&query_words, &object);
                let score = subject_match * 3 + relation_match * 2 + object_match;
                (score > 0).then_some((score, entry))
            })
            .collect();
        scored.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| right.1.id.cmp(&left.1.id))
        });
        scored
            .into_iter()
            .take(limit)
            .map(|(_, entry)| entry)
            .collect()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        fs::rename(temporary, path)
    }

    fn insert(
        &mut self,
        subject: &str,
        relation: &str,
        object: &str,
        source: &str,
        supersedes: Option<u64>,
    ) -> io::Result<u64> {
        for field in [subject, relation, object, source] {
            check_field(field)?;
        }
        if self.entries.len() >= MAX_ENTRIES {
            return Err(invalid("knowledge map is full"));
        }
        let id = self.entries.last().map_or(Ok(1), |entry| {
            entry
                .id
                .checked_add(1)
                .ok_or_else(|| invalid("knowledge ID limit reached"))
        })?;
        self.entries.push(Claim {
            id,
            subject: subject.trim().into(),
            relation: relation.trim().into(),
            object: object.trim().into(),
            source: source.trim().into(),
            supersedes,
            retracted_by: None,
        });
        Ok(id)
    }

    fn active_index(&self, id: u64) -> io::Result<usize> {
        self.entries
            .iter()
            .position(|entry| entry.id == id && entry.retracted_by.is_none())
            .ok_or_else(|| invalid("no active claim with that ID"))
    }
}

fn words(text: &str) -> BTreeSet<String> {
    crate::storage::relevant_words(text)
}

fn lexical_overlap(left: &BTreeSet<String>, right: &BTreeSet<String>) -> usize {
    left.iter()
        .filter(|word| right.iter().any(|candidate| lexical_match(word, candidate)))
        .count()
}

fn lexical_match(left: &str, right: &str) -> bool {
    left == right
        || (left.chars().count() >= 4
            && right.chars().count() >= 4
            && (left.starts_with(right) || right.starts_with(left)))
}

fn check_field(field: &str) -> io::Result<()> {
    let length = field.trim().chars().count();
    if length == 0
        || length > MAX_FIELD_CHARS
        || field.chars().any(char::is_control)
        || field.contains("<|")
    {
        return Err(invalid(
            "claim fields must contain 1 to 500 characters without control or model delimiter text",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::KnowledgeMap;

    #[test]
    fn revisions_and_retractions_change_retrieval_across_restarts() {
        let path = crate::test_support::unique_temp_path("community-map", "json");
        let mut map = KnowledgeMap::load(&path).unwrap();
        let id = map
            .add("Willow", "grows beside", "rivers", "field note A")
            .unwrap();
        map.add("Moon", "orbits", "Earth", "field note B").unwrap();
        assert_eq!(map.relevant("Tell me about willow", 5)[0].id, id);
        let replacement = map
            .revise(id, "Willow", "grows beside", "streams", "field note C")
            .unwrap();
        map.save(&path).unwrap();
        let mut loaded = KnowledgeMap::load(&path).unwrap();
        assert_eq!(
            loaded.relevant("What grows near willow?", 5)[0].id,
            replacement
        );
        assert!(
            loaded
                .relevant("What grows near willow?", 5)
                .iter()
                .all(|entry| entry.object != "rivers")
        );
        loaded.retract(replacement, "field note D").unwrap();
        assert!(loaded.relevant("What grows near willow?", 5).is_empty());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn common_words_do_not_retrieve_the_hagia_sophia_claim_for_element_questions() {
        let mut map = KnowledgeMap::default();
        map.add(
            "the Hagia Sophia",
            "is taller than",
            "the Great Pyramid of Giza",
            "user observation",
        )
        .unwrap();
        for query in [
            "Do you want to tell me who the most recent discoverer of an element was again?",
            "Much better. Thank you for being honest about being unsure :)",
            "the",
            "What is the atomic number of Oganesson?",
        ] {
            assert!(map.relevant(query, 5).is_empty(), "{query}");
        }
        assert_eq!(map.relevant("How tall is Hagia Sophia?", 5).len(), 1);
    }

    #[test]
    fn self_referential_questions_retrieve_user_claims_with_related_word_forms() {
        let mut map = KnowledgeMap::default();
        let name = map
            .add("the user", "is named", "Ryan", "user message 1")
            .unwrap();
        map.add("the user", "lives in", "Bristol", "user message 2")
            .unwrap();
        let result = map.relevant("Do you know my name?", 5);
        assert_eq!(result[0].id, name);
    }
}
