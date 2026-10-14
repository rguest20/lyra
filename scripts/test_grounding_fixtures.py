import importlib.util
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("grounding", Path(__file__).with_name("run_grounding_fixtures.py"))
grounding = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(grounding)


class GroundingFixtureTests(unittest.TestCase):
    def case(self):
        return {"id": "observatory", "question": "Who founded it?", "evidence": [
            {"id": "source", "origin": "reference.example", "passage": "Ada Voss founded the observatory."}]}

    def test_gold_and_extra_source_fields_cannot_enter_runtime(self):
        for field in ("expected", "rubric", "answer"):
            case = self.case()
            case[field] = "gold sentinel"
            with self.assertRaises(ValueError):
                grounding.fixture_input(case)
        case = self.case()
        case["evidence"][0]["expected"] = "gold sentinel"
        with self.assertRaises(ValueError):
            grounding.fixture_input(case)
        payload = grounding.fixture_input(self.case())
        self.assertNotIn("id", payload)
        self.assertNotIn("gold sentinel", json.dumps(payload))

    def test_case_paths_and_duplicate_sources_are_rejected(self):
        case = self.case()
        case["id"] = "../outside"
        with self.assertRaises(ValueError):
            grounding.fixture_input(case)
        case = self.case()
        case["evidence"] *= 2
        with self.assertRaises(ValueError):
            grounding.fixture_input(case)

    def test_archive_audit_checks_full_source_retention_and_network_isolation(self):
        payload = grounding.fixture_input(self.case())
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "learning.db"
            with sqlite3.connect(path) as database:
                database.execute("CREATE TABLE research_sources(url TEXT, body TEXT)")
                database.execute("CREATE TABLE host_request_log(id INTEGER)")
                database.execute("INSERT INTO research_sources VALUES (?,?)", (
                    "https://fixture.invalid/source-1", payload["evidence"][0]["passage"]))
            self.assertTrue(grounding.audit_database(path, payload)["transport_valid"])
            with sqlite3.connect(path) as database:
                database.execute("UPDATE research_sources SET body='truncated'")
            self.assertFalse(grounding.audit_database(path, payload)["archive_complete"])
            with sqlite3.connect(path) as database:
                database.execute("UPDATE research_sources SET body=?", (payload["evidence"][0]["passage"],))
                database.execute("INSERT INTO host_request_log VALUES(1)")
            audit = grounding.audit_database(path, payload)
            self.assertTrue(audit["archive_complete"])
            self.assertFalse(audit["transport_valid"])


if __name__ == "__main__":
    unittest.main()
