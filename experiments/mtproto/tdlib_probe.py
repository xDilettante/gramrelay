"""Offline probe of the locally built, pinned TDLib C ABI (no login)."""
import argparse
from contextlib import ExitStack
import ctypes
import json
import os
from pathlib import Path
import shutil
import time

ROOT = Path(__file__).resolve().parents[2]


def probe(library_path):
    library_path = library_path.resolve(strict=True)
    if not library_path.is_relative_to(ROOT.resolve() / "target/mtproto-spike/tdlib-build"):
        raise ValueError("probe only loads the project-local TDLib build")
    with ExitStack() as directories:
        if os.name == "nt":
            for directory in [library_path.parent, ROOT / "target/mtproto-spike/dependencies/ucrt64/bin", Path(shutil.which("g++")).parent]:
                directories.enter_context(os.add_dll_directory(directory))
        library = ctypes.CDLL(str(library_path))
        library.td_execute.argtypes = [ctypes.c_char_p]
        library.td_execute.restype = ctypes.c_char_p
        library.td_create_client_id.argtypes = []
        library.td_create_client_id.restype = ctypes.c_int
        library.td_send.argtypes = [ctypes.c_int, ctypes.c_char_p]
        library.td_send.restype = None
        library.td_receive.argtypes = [ctypes.c_double]
        library.td_receive.restype = ctypes.c_char_p

        def execute(request):
            # c_char_p copies the returned NUL-terminated bytes immediately;
            # no borrowed native pointer survives the next receive/execute.
            result = library.td_execute(json.dumps(request).encode())
            assert result is not None
            assert len(result) <= 1024 * 1024
            return json.loads(result)

        assert execute({"@type": "setLogVerbosityLevel", "new_verbosity_level": 0})["@type"] == "ok"
        entities = execute({"@type": "getTextEntities", "text": "🙂 https://example.com"})
        assert entities["@type"] == "textEntities"
        assert any(entity["offset"] == 3 and entity["length"] == 19 and entity["type"]["@type"] == "textEntityTypeUrl" for entity in entities["entities"])
        client = library.td_create_client_id()
        assert client > 0

        def send(request):
            library.td_send(client, json.dumps(request).encode())

        try:
            send({"@type": "getAuthorizationState", "@extra": "offline-state"})
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                data = library.td_receive(0.1)
                if data is None:
                    continue
                response = json.loads(data)
                if response.get("@client_id") != client:
                    continue
                if response.get("@extra") == "offline-state":
                    assert response["@type"] == "authorizationStateWaitTdlibParameters"
                    break
            else:
                raise RuntimeError("TDLib authorization-state response deadline")
        finally:
            send({"@type": "close", "@extra": "offline-close"})
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                data = library.td_receive(0.1)
                if data is not None:
                    response = json.loads(data)
                    if response.get("@client_id") == client and response["@type"] == "updateAuthorizationState" and response["authorization_state"]["@type"] == "authorizationStateClosed":
                        break
            else:
                raise RuntimeError("TDLib explicit close deadline")
        print("TDLib C ABI: synchronous UTF-16 entities, @extra correlation, credential-free wait state and explicit close verified; no setTdlibParameters or sign-in sent.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("library", type=Path)
    probe(parser.parse_args().library)
