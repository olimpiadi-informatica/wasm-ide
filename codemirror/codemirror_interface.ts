import {basicSetup} from "codemirror";
import {EditorView, keymap} from "@codemirror/view";
import {ChangeSet, Compartment, Prec, EditorState, Facet, Extension, Text, TransactionSpec} from "@codemirror/state";
import {StreamLanguage} from "@codemirror/language";
import {pascal} from "@codemirror/legacy-modes/mode/pascal";
import {cpp} from "@codemirror/lang-cpp";
import {python} from "@codemirror/lang-python";
import {rust} from "@codemirror/lang-rust";
import {go} from "@codemirror/lang-go";
import {java} from "@codemirror/lang-java";
import {javascript} from "@codemirror/lang-javascript";
import {csharp} from "@replit/codemirror-lang-csharp";
import {emacs} from "@replit/codemirror-emacs";
import {vim} from "@replit/codemirror-vim";
import {indentWithTab} from "@codemirror/commands"
import {solarizedLight, solarizedDark} from "@uiw/codemirror-theme-solarized";
import {LSPClient, LSPPlugin, Transport, Workspace, WorkspaceFile, findReferencesKeymap, formatKeymap, jumpToDefinitionKeymap, languageServerExtensions, renameKeymap} from "@codemirror/lsp-client";

export class LSEventHandler implements Transport {
  subscribers = new Set<(value: string) => void>();

  constructor(public client: LSPClient, public sendMessage: (msg: string) => void) {}

  send(msg: string): void {
    this.sendMessage(msg);
  }

  subscribe(handler: (value: string) => void): void {
    this.subscribers.add(handler);
  }

  unsubscribe(handler: (value: string) => void): void {
    this.subscribers.delete(handler);
  }

  ready() {
    this.client.connect(this);
  }

  stopping() {
    this.client.disconnect();
  }

  message(msg: string) {
    this.subscribers.forEach((handler) => handler(msg));
  }
}

const useLast = (values: readonly any[]) => values.reduce((_, v) => v, "");
const languageId = Facet.define<string, string>({combine: useLast});

class MyWorkspaceFile implements WorkspaceFile {
  version = 0;

  constructor(
    readonly editor: CM6Editor,
    readonly filename: string,
    readonly uri: string,
    public languageId: string,
    public doc: Text,
  ) {}

  getView(): EditorView | null {
    return this.editor.filename === this.filename ? this.editor.view : null;
  }
}

class MyWorkspace extends Workspace {
  files: MyWorkspaceFile[] = [];
  connectedToServer = false;

  constructor(client: LSPClient, readonly editor: CM6Editor) {
    super(client);
  }

  addFile(filename: string, uri: string, languageID: string, doc: Text) {
    const existing = this.getFile(uri) as MyWorkspaceFile | null;
    if (existing) {
      existing.languageId = languageID;
      return;
    }
    const file = new MyWorkspaceFile(this.editor, filename, uri, languageID, doc);
    this.files.push(file);
    if (this.connectedToServer) this.client.didOpen(file);
  }

  retainFiles(filenames: Set<string>) {
    for (const file of this.files) {
      if (!filenames.has(file.filename) && this.connectedToServer) {
        this.client.didClose(file.uri);
      }
    }
    this.files = this.files.filter((file) => filenames.has(file.filename));
  }

  setLanguage(languageID: string) {
    for (const file of this.files) file.languageId = languageID;
  }

  syncFiles() {
    const updates: {file: MyWorkspaceFile, prevDoc: Text, changes: ChangeSet}[] = [];
    if (!this.connectedToServer) return updates;
    for (const file of this.files) {
      const state = this.editor.stateFor(file.filename);
      if (!state || state.doc === file.doc) continue;

      const prevDoc = file.doc;
      const changes = ChangeSet.of({
        from: 0,
        to: prevDoc.length,
        insert: state.doc.toString(),
      }, prevDoc.length);
      file.doc = state.doc;
      file.version++;
      updates.push({file, prevDoc, changes});

      const view = file.getView();
      if (view) LSPPlugin.get(view)?.clear();
    }
    return updates;
  }

  openFile(uri: string, languageID: string, view: EditorView) {
    const file = this.getFile(uri) as MyWorkspaceFile | null;
    if (file) {
      file.languageId = languageID;
    } else {
      this.addFile(this.editor.filename, uri, languageID, view.state.doc);
    }
  }

  closeFile(_uri: string, _view: EditorView) {
    // A tab switch destroys the active view plugin, but the document remains
    // open in this workspace.
  }

  connected() {
    this.connectedToServer = true;
    for (const file of this.files) {
      const state = this.editor.stateFor(file.filename);
      if (state) file.doc = state.doc;
    }
    super.connected();
  }

  disconnected() {
    this.connectedToServer = false;
  }

  displayFile(uri: string): Promise<EditorView | null> {
    return Promise.resolve(this.editor.displayFile(uri));
  }

  updateFile(uri: string, update: TransactionSpec) {
    this.editor.updateFile(uri, update);
  }
}

export class CM6Editor {
  language = new Compartment();
  keyboardMode = new Compartment();
  theme = new Compartment();
  isReadOnly = new Compartment();

  execCallback: () => void = () => {};
  onchangeCallback: (filename: string) => void = () => {};
  execKeyBinding = Prec.highest(
    keymap.of([
      {
        key: "Mod-Enter",
        run: () => {
          this.execCallback();
          return true;
        },
      },
    ]),
  );
  view: EditorView;

  lspClient: LSPClient;
  lspWorkspace: MyWorkspace;
  lspPlugin = new Compartment();
  states = new Map<string, EditorState>();
  filename = "";
  languageID = "";
  languageExtension: Extension = [];
  keyboardExtension: Extension = [];
  themeExtension: Extension = solarizedLight;
  readOnly = false;
  openFileCallback: (filename: string) => void = () => {};

  constructor(element: HTMLElement) {
    this.lspClient = new LSPClient({
      rootUri: "file:///workdir",
      extensions: languageServerExtensions(),
      workspace: (client: LSPClient) => {
        this.lspWorkspace = new MyWorkspace(client, this);
        return this.lspWorkspace;
      },
    });
    this.view = new EditorView({
      state: this.createState(),
      parent: element,
    });
  }

  createState(doc = "", fileUri = this.uriFor(this.filename)) {
    return EditorState.create({
      doc,
      extensions: [
        basicSetup,

        keymap.of([indentWithTab]),
        this.keyboardMode.of(this.keyboardExtension),
        this.execKeyBinding,
        this.theme.of(this.themeExtension),
        this.language.of([this.languageExtension, languageId.of(this.languageID)]),
        this.isReadOnly.of(EditorState.readOnly.of(this.readOnly)),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            this.onchangeCallback(this.filename);
          }
        }),

        // LSP related plugins
        this.lspPlugin.of(this.languageServerExtension(fileUri)),
        keymap.of(formatKeymap),
        keymap.of(renameKeymap),
        keymap.of(jumpToDefinitionKeymap),
        keymap.of(findReferencesKeymap),
      ],
    });
  }

  languageServerExtension(fileUri = this.uriFor(this.filename)): Extension {
    return fileUri && this.languageID
      ? [this.lspClient.plugin(fileUri, this.languageID)]
      : [];
  }

  configureState(state: EditorState) {
    return state.update({
      effects: [
        this.keyboardMode.reconfigure(this.keyboardExtension),
        this.theme.reconfigure(this.themeExtension),
        this.language.reconfigure([this.languageExtension, languageId.of(this.languageID)]),
        this.isReadOnly.reconfigure(EditorState.readOnly.of(this.readOnly)),
        this.lspPlugin.reconfigure(this.languageServerExtension()),
      ],
    }).state;
  }

  setLanguage(lang: string) {
    if (lang === "C") {
      this.languageExtension = cpp();
      this.languageID = "c";
    } else if (lang === "C++") {
      this.languageExtension = cpp();
      this.languageID = "cpp";
    } else if (lang === "Python3") {
      this.languageExtension = python();
      this.languageID = "python";
    } else if (lang === "Rust") {
      this.languageExtension = rust();
      this.languageID = "rust";
    } else if (lang === "Pascal") {
      this.languageExtension = StreamLanguage.define(pascal);
      this.languageID = "pascal";
    } else if (lang === "Go") {
      this.languageExtension = go();
      this.languageID = "go";
    } else if (lang === "Java") {
      this.languageExtension = java();
      this.languageID = "java";
    } else if (lang === "JavaScript") {
      this.languageExtension = javascript();
      this.languageID = "javascript";
    } else if (lang === "C#") {
      this.languageExtension = csharp();
      this.languageID = "csharp";
    } else {
      this.languageExtension = [];
      this.languageID = "";
    }
    this.lspWorkspace.setLanguage(this.languageID);
    this.view.dispatch({
      effects: [
        this.language.reconfigure([this.languageExtension, languageId.of(this.languageID)]),
        this.lspPlugin.reconfigure(this.languageServerExtension()),
      ],
    });
  }

  setFile(filename: string) {
    if (filename === this.filename) return;
    this.lspClient.sync();
    if (this.filename) this.states.set(this.filename, this.view.state);

    this.filename = filename;
    const fileUri = this.uriFor(filename);
    const state = filename ? this.states.get(filename) : undefined;
    const nextState = state || this.createState("", fileUri);
    if (filename) {
      this.lspWorkspace.addFile(filename, fileUri, this.languageID, nextState.doc);
    }
    this.view.setState(this.configureState(nextState));
  }

  setFiles(files: [string, string][]) {
    const filenames = new Set(files.map(([filename]) => filename));
    this.lspWorkspace.retainFiles(filenames);

    for (const [filename, text] of files) {
      const uri = this.uriFor(filename);
      let state = this.stateFor(filename);
      if (!state) {
        state = this.createState(text, uri);
        this.states.set(filename, state);
      }
      this.lspWorkspace.addFile(filename, uri, this.languageID, state.doc);
    }

    for (const filename of Array.from(this.states.keys())) {
      if (!filenames.has(filename)) this.states.delete(filename);
    }
  }

  stateFor(filename: string): EditorState | undefined {
    return filename === this.filename ? this.view.state : this.states.get(filename);
  }

  uriFor(filename: string): string {
    const basename = filename.split("/").pop() || "";
    return basename ? new URL(encodeURIComponent(basename), "file:///workdir/").href : "";
  }

  displayFile(uri: string): EditorView | null {
    const file = this.lspWorkspace.getFile(uri) as MyWorkspaceFile | null;
    if (!file) return null;
    this.setFile(file.filename);
    this.openFileCallback(file.filename);
    return this.view;
  }

  updateFile(uri: string, update: TransactionSpec) {
    const file = this.lspWorkspace.getFile(uri) as MyWorkspaceFile | null;
    if (!file) return;
    if (file.filename === this.filename) {
      this.view.dispatch(update);
      return;
    }
    const state = this.states.get(file.filename);
    if (!state) return;
    const updated = state.update(update).state;
    this.states.set(file.filename, updated);
    this.onchangeCallback(file.filename);
    this.lspClient.sync();
  }

  setDark(dark: boolean) {
    this.themeExtension = dark ? solarizedDark : solarizedLight;
    this.view.dispatch({
      effects: this.theme.reconfigure(this.themeExtension),
    });
  }

  setReadOnly(isReadonly: boolean) {
    this.readOnly = isReadonly;
    this.view.dispatch({
      effects: this.isReadOnly.reconfigure(EditorState.readOnly.of(isReadonly)),
    });
  }

  setKeymap(keymap: string) {
    if (keymap === "vim") {
      this.keyboardExtension = vim();
    } else if (keymap === "emacs") {
      this.keyboardExtension = emacs();
    } else {
      this.keyboardExtension = [];
    }
    this.view.dispatch({effects: this.keyboardMode.reconfigure(this.keyboardExtension)});
  }

  setExec(exec: () => void) {
    this.execCallback = exec;
  }

  setOnchange(onchange: (filename: string) => void) {
    this.onchangeCallback = onchange;
  }

  setOpenFile(openFile: (filename: string) => void) {
    this.openFileCallback = openFile;
  }

  setText(text: string) {
    if (text === this.view.state.doc.toString()) return;
    this.view.dispatch({
      changes: {
        from: 0,
        to: this.view.state.doc.length,
        insert: text,
      },
    });
  }

  getText(): string {
    return this.view.state.doc.toString();
  }

  getFileText(filename: string): string {
    return this.stateFor(filename)?.doc.toString() || "";
  }

  setLanguageServer(sendMessage: (msg: string) => void): LSEventHandler {
    return new LSEventHandler(this.lspClient, sendMessage);
  }
}
