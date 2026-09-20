"""Compile C/C++ consumers and read their output through the Python binding."""
import os
from pathlib import Path
import subprocess
import shlex
import tempfile
import unittest
from pqsio import Reader

PROJECT=Path(__file__).resolve().parents[1]
OUTPUT=PROJECT/"tests/output"
OUTPUT.mkdir(exist_ok=True)
CC = shlex.split(os.environ.get("CC", "cc"))
CXX = shlex.split(os.environ.get("CXX", "c++"))

class NativeABI(unittest.TestCase):
    def test_compression_c_cpp(self):
        lib=Path(os.environ["PQSIO_LIBRARY"]).resolve()
        for compiler,source,standard in ((CC,"compression.c","c11"),(CXX,"compression.cpp","c++17")):
            with self.subTest(source=source), tempfile.TemporaryDirectory(dir=OUTPUT) as tmp:
                exe=Path(tmp)/"compression"
                subprocess.run(compiler + ["-std="+standard,"-Wall","-Wextra","-Werror",
                    "-I",str(PROJECT/"include"),str(PROJECT/"tests"/source),
                    "-L",str(lib.parent),"-lpqsio","-Wl,-rpath,"+str(lib.parent),"-o",str(exe)],
                    check=True,capture_output=True,text=True,timeout=60)
                paths=[Path(tmp)/"sync",Path(tmp)/"parallel"]
                subprocess.run([str(exe),*map(str,paths)],check=True,capture_output=True,text=True,timeout=60)
                for path in paths:
                    for q in (0,1):
                        with Reader(path,q) as reader:
                            rows=[r for b in reader.iter_batches() for r in b]
                            self.assertEqual([(r.read_id,r.pos1,r.pos2,r.mapq) for r in rows],
                                             [("compressed",1,2,60)])

    def test_native_import_api(self):
        lib=Path(os.environ["PQSIO_LIBRARY"]).resolve()
        with tempfile.TemporaryDirectory(dir=OUTPUT) as tmp:
            source=Path(tmp)/"input.paf"
            source.write_text("read\t20\t0\t10\t+\ta\t100\t0\t10\t10\t10\t30\n"
                              "read\t20\t10\t20\t-\ta\t100\t20\t30\t10\t10\t20\n")
            exe=Path(tmp)/"import"
            subprocess.run(CC + ["-std=c11","-Wall","-Wextra","-Werror",
                "-I",str(PROJECT/"include"),str(PROJECT/"tests/import.c"),
                "-L",str(lib.parent),"-lpqsio","-Wl,-rpath,"+str(lib.parent),"-o",str(exe)],
                check=True,capture_output=True,text=True,timeout=60)
            output=Path(tmp)/"pairs"
            subprocess.run([str(exe),str(source),str(output)],
                check=True,capture_output=True,text=True,timeout=60)
            with Path(str(output)+".cool").open("rb") as cooler:
                self.assertEqual(cooler.read(8), b"\x89HDF\r\n\x1a\n")
            with Reader(output) as reader:
                rows=[r for batch in reader.iter_batches() for r in batch]
                self.assertEqual([(r.read_id,r.pos1,r.pos2,r.mapq) for r in rows],
                                 [("read:0:1",1,21,20)])

    def test_columnar_c_cpp(self):
        lib=Path(os.environ["PQSIO_LIBRARY"]).resolve()
        for compiler,source,standard in ((CC,"columns.c","c11"),(CXX,"columns.cpp","c++17")):
            with self.subTest(source=source), tempfile.TemporaryDirectory(dir=OUTPUT) as tmp:
                exe=Path(tmp)/"columns"
                subprocess.run(compiler + ["-std="+standard,"-Wall","-Wextra","-Werror",
                    "-I",str(PROJECT/"include"),str(PROJECT/"tests"/source),
                    "-L",str(lib.parent),"-lpqsio","-Wl,-rpath,"+str(lib.parent),"-o",str(exe)],
                    check=True,capture_output=True,text=True,timeout=60)
                paths=[Path(tmp)/"pairs",Path(tmp)/"concat"]
                subprocess.run([str(exe),*map(str,paths)],check=True,capture_output=True,text=True,timeout=60)
                for path in paths:
                    with Reader(path) as r:
                        self.assertTrue(list(r.iter_columns()))

    def test_cpp_parallel_api(self):
        lib=Path(os.environ["PQSIO_LIBRARY"]).resolve()
        with tempfile.TemporaryDirectory(dir=OUTPUT) as tmp:
            exe=Path(tmp)/"parallel"
            subprocess.run(CXX + ["-std=c++17","-pthread","-Wall","-Wextra","-Werror","-I",str(PROJECT/"include"),
                str(PROJECT/"tests/parallel.cpp"),"-L",str(lib.parent),"-lpqsio","-Wl,-rpath,"+str(lib.parent),
                "-o",str(exe)],check=True,capture_output=True,text=True,timeout=60)
            path=Path(tmp)/"parallel.pqs"
            subprocess.run([str(exe),str(path)],check=True,capture_output=True,text=True,timeout=60)
            with Reader(path) as reader:
                self.assertEqual(len([r for b in reader.iter_batches() for r in b]),3)

    def test_cpp_bulk_api(self):
        lib=Path(os.environ["PQSIO_LIBRARY"]).resolve()
        with tempfile.TemporaryDirectory(dir=OUTPUT) as tmp:
            exe=Path(tmp)/"bulk"
            subprocess.run(CXX + ["-std=c++17","-Wall","-Wextra","-Werror","-I",str(PROJECT/"include"),
                str(PROJECT/"tests/bulk.cpp"),"-L",str(lib.parent),"-lpqsio","-Wl,-rpath,"+str(lib.parent),
                "-o",str(exe)],check=True,capture_output=True,text=True,timeout=60)
            subprocess.run([str(exe),str(Path(tmp)/"concat.pqs")],check=True,capture_output=True,text=True,timeout=60)

    def test_c_and_cpp_read_write_both_formats(self):
        lib=Path(os.environ["PQSIO_LIBRARY"]).resolve()
        for compiler,source,standard in ((CC,"smoke.c","c11"),(CXX,"smoke.cpp","c++17")):
            with self.subTest(language=source), tempfile.TemporaryDirectory(dir=OUTPUT) as tmp:
                exe=Path(tmp)/"smoke"
                subprocess.run(compiler + ["-std="+standard,"-Wall","-Wextra","-Werror",
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
