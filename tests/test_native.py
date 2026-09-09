"""Compile C/C++ consumers and read their output through the Python binding."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from pqsio import Reader

PROJECT=Path(__file__).resolve().parents[1]
OUTPUT=PROJECT/"tests/output"
OUTPUT.mkdir(exist_ok=True)

class NativeABI(unittest.TestCase):
    def test_c_and_cpp_read_write_both_formats(self):
        lib=Path(os.environ["PQSIO_LIBRARY"]).resolve()
        for compiler,source,standard in (("cc","smoke.c","c11"),("c++","smoke.cpp","c++17")):
            with self.subTest(language=source), tempfile.TemporaryDirectory(dir=OUTPUT) as tmp:
                exe=Path(tmp)/"smoke"
                subprocess.run([compiler,"-std="+standard,"-Wall","-Wextra","-Werror",
                    "-I",str(PROJECT/"include"),str(PROJECT/"tests"/source),
                    "-L",str(lib.parent),"-lpqsio","-Wl,-rpath,"+str(lib.parent),"-o",str(exe)],
                    check=True,capture_output=True,text=True,timeout=60)
                paths=[Path(tmp)/"pairs.pqs",Path(tmp)/"concat.pqs"]
                subprocess.run([str(exe),*map(str,paths)],check=True,capture_output=True,text=True,timeout=60)
                for kind,path in zip(("pairs","concat"),paths):
                    with Reader(path) as reader:
                        self.assertEqual(reader.kind,kind)
                        self.assertEqual(reader.contigs,[("chr1",100)])
                        rows=[r for b in reader.iter_batches() for r in b]
                        self.assertEqual(len(rows),2 if source=="smoke.c" and kind=="concat" else 1)

if __name__=="__main__":
    unittest.main()
