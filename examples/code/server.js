// JavaScript example for syntax highlighting demo.

class FileServer {
  constructor(root, port = 8080) {
    this.root = root;
    this.port = port;
  }

  async start() {
    const entries = await this.scan(this.root);
    console.log(`Serving ${entries.length} entries on :${this.port}`);
    return entries.map((e) => e.name);
  }

  async scan(dir) {
    return [{ name: "hello.rs", size: 128 }];
  }
}

const server = new FileServer("./public");
server.start().then((names) => names.forEach((n) => console.log(`  ${n}`)));
