// Runs the wasm converter off the main thread. One worker per conversion.
self.onmessage = async (event) => {
  const { data } = event.data; // ArrayBuffer with the DjVu file
  try {
    const imports = {
      djvu2pdf: { progress: (done, total) => self.postMessage({ type: "progress", done, total }) },
    };
    const source = fetch("djvu2pdf.wasm");
    let instance;
    try {
      ({ instance } = await WebAssembly.instantiateStreaming(source, imports));
    } catch {
      // servers without the application/wasm MIME type
      const bytes = await (await fetch("djvu2pdf.wasm")).arrayBuffer();
      ({ instance } = await WebAssembly.instantiate(bytes, imports));
    }
    const e = instance.exports;
    const input = new Uint8Array(data);
    const ptr = e.input_alloc(input.length);
    new Uint8Array(e.memory.buffer, ptr, input.length).set(input);
    let status;
    try {
      status = e.convert(ptr, input.length);
    } catch (trap) {
      const msg = new TextDecoder().decode(new Uint8Array(e.memory.buffer, e.panic_ptr(), e.panic_len()));
      throw new Error(msg || String(trap));
    }
    const result = new Uint8Array(e.memory.buffer, e.result_ptr(), e.result_len()).slice();
    if (status !== 0) throw new Error(new TextDecoder().decode(result));
    const stats = JSON.parse(new TextDecoder().decode(new Uint8Array(e.memory.buffer, e.stats_ptr(), e.stats_len())));
    self.postMessage({ type: "done", pdf: result.buffer, stats }, [result.buffer]);
  } catch (err) {
    self.postMessage({ type: "error", message: err.message || String(err) });
  }
};
