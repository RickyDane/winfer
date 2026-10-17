import datetime
import os
import tempfile
import unittest

from importer.csvimport import CsvImportError, load, parse

S = {"id": "int", "name": "str", "score": "float", "active": "bool", "joined": "date"}


class Basics(unittest.TestCase):
    def test_simple(self):
        rows = parse("id,name,score,active,joined\n1,Ann,2.5,yes,2024-02-29\n", S)
        self.assertEqual(rows, [{"id": 1, "name": "Ann", "score": 2.5, "active": True, "joined": datetime.date(2024, 2, 29)}])

    def test_no_trailing_newline_and_crlf(self):
        rows = parse("id,name\r\n1,a\r\n2,b", {"id": "int", "name": "str"})
        self.assertEqual(rows, [{"id": 1, "name": "a"}, {"id": 2, "name": "b"}])

    def test_quotes(self):
        text = 'name,id\n"Smith, J",1\n"say ""hi""",2\n"multi\nline\r\nvalue",3\n"",4\n'
        rows = parse(text, {"id": "int", "name": "str"})
        self.assertEqual([r["name"] for r in rows], ["Smith, J", 'say "hi"', "multi\nline\nvalue", ""])
        self.assertEqual([r["id"] for r in rows], [1, 2, 3, 4])

    def test_bom_and_header_whitespace(self):
        rows = parse("﻿ id , name \n7,x\n", {"id": "int", "name": "str"})
        self.assertEqual(rows, [{"id": 7, "name": "x"}])

    def test_blank_lines(self):
        rows = parse("\n\nid,name\n\n1,a\n\r\n\n2,b\n\n", {"id": "int", "name": "str"})
        self.assertEqual(rows, [{"id": 1, "name": "a"}, {"id": 2, "name": "b"}])

    def test_extra_columns_dropped_and_order(self):
        rows = parse("x,name,id,y\n9,a,1,8\n", {"id": "int", "name": "str"})
        self.assertEqual(rows, [{"id": 1, "name": "a"}])

    def test_str_not_stripped(self):
        rows = parse("name,n\n  pad  ,\n", {"name": "str", "n": "int"})
        self.assertEqual(rows, [{"name": "  pad  ", "n": None}])

    def test_empty_typed(self):
        rows = parse("id,name,score,active,joined\n , , ,  ,\n", S)
        self.assertEqual(rows, [{"id": None, "name": " ", "score": None, "active": None, "joined": None}])

    def test_conversions(self):
        rows = parse("i,f,b\n +3 ,1e3,TRUE\n-12,-0.5,No\n0007,.5,0\n", {"i": "int", "f": "float", "b": "bool"})
        self.assertEqual(rows, [{"i": 3, "f": 1000.0, "b": True}, {"i": -12, "f": -0.5, "b": False}, {"i": 7, "f": 0.5, "b": False}])

    def test_header_only(self):
        self.assertEqual(parse("id,name\n", {"id": "int", "name": "str"}), [])


class Errors(unittest.TestCase):
    def err(self, text, schema):
        with self.assertRaises(CsvImportError) as cm:
            parse(text, schema)
        return cm.exception

    def test_missing_column(self):
        e = self.err("\n\nid,nme\n1,a\n", {"id": "int", "name": "str"})
        self.assertEqual(e.line, 3)
        self.assertIn("name", str(e))

    def test_field_count(self):
        e = self.err("id,name\n1,a\n2\n", {"id": "int", "name": "str"})
        self.assertEqual(e.line, 3)
        e = self.err("id,name\n1,a,b\n", {"id": "int", "name": "str"})
        self.assertEqual(e.line, 2)

    def test_line_numbers_after_multiline(self):
        e = self.err('id,name\n1,"a\nb\nc"\n\n2,x\n3.5,y\n', {"id": "int", "name": "str"})
        self.assertEqual(e.line, 7)
        self.assertIn("id", str(e))

    def test_line_of_multiline_record_start(self):
        e = self.err('id,name\n1,ok\nzz,"a\nb"\n', {"id": "int", "name": "str"})
        self.assertEqual(e.line, 3)

    def test_bad_values(self):
        for kind, bad in [("int", "1.5"), ("int", "1e3"), ("int", "abc"), ("float", "nan"), ("float", "-inf"),
                          ("float", "x"), ("bool", "maybe"), ("bool", "t"), ("date", "2023-02-29"),
                          ("date", "2023-1-05"), ("date", "05/01/2023")]:
            with self.subTest(kind=kind, bad=bad):
                e = self.err(f"col\n{bad}\n", {"col": kind})
                self.assertEqual(e.line, 2)
                self.assertIn("col", str(e))


class Load(unittest.TestCase):
    def test_load_bom_file(self):
        fd, path = tempfile.mkstemp(suffix=".csv")
        os.close(fd)
        try:
            with open(path, "wb") as f:
                f.write("id,name\r\n1,\"Zoë\r\nRow\"\r\n2,b\r\n".encode("utf-8-sig"))
            self.assertEqual(load(path, {"id": "int", "name": "str"}), [{"id": 1, "name": "Zoë\nRow"}, {"id": 2, "name": "b"}])
        finally:
            os.remove(path)


if __name__ == "__main__":
    unittest.main()
