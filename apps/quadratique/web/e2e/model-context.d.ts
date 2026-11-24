/** The experimental WebMCP host API that Chromium exposes on `document` behind `--enable-features=WebMCP`. */
interface ModelContextTool {
  readonly name: string;
}

interface ModelContext {
  readonly getTools: () => Promise<readonly ModelContextTool[]>;
  readonly executeTool: (tool: ModelContextTool, input: object) => Promise<string>;
}

interface Document {
  readonly modelContext?: ModelContext;
}
