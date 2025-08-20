import {basicSetup} from "codemirror";
import {
  EditorView,
  keymap,
  PluginValue,
  ViewPlugin,
  ViewUpdate,
  hoverTooltip,
  Tooltip,
} from "@codemirror/view";
import {setDiagnostics} from "@codemirror/lint";
import {Compartment, Prec, EditorState, Facet, Text} from "@codemirror/state";
import {cpp} from "@codemirror/lang-cpp";
import {python} from "@codemirror/lang-python";
import {emacs} from "@replit/codemirror-emacs";
import {vim} from "@replit/codemirror-vim";
import {indentWithTab} from "@codemirror/commands"
import {solarizedLight, solarizedDark} from "@uiw/codemirror-theme-solarized";
import {
  autocompletion,
  Completion,
  CompletionContext,
  CompletionResult,
} from "@codemirror/autocomplete";

interface Position {
  line: number;
  character: number;
}

interface Range {
  start: Position;
  end: Position;
}

function mapPosWithDoc(doc: Text, pos: Position): number {
  if (pos.line >= doc.lines) return 0;
  const offset = doc.line(pos.line + 1).from + pos.character;
  return offset <= doc.length ? offset : 0;
}

function mapOffsetWithDoc(doc: Text, offset: number): Position {
  const line = doc.lineAt(offset);
  return {
    line: line.number - 1,
    character: offset - line.from,
  };
}

export class LSEventHandler {
  isReady = false;
  sendCount = 0;
  pending = new Object();
  plugin: LSPlugin = null;
  capabilities: any;
  documentIsSynced = false;

  constructor(public sendMessage: (msg: string) => void) {}

  getUri(): string {
    let language = this.plugin.view.state.facet(languageId);
    return URI + languageToExtension[language];
  }

  ready() {
    (async () => {
      // TODO: add client capabilities?
      let init = await this.request("initialize", {
        "capabilities": {},
      });
      this.capabilities = init["capabilities"];
      this.notify("initialized", {});
      let language = this.plugin.view.state.facet(languageId);
      this.documentIsSynced = true;
      this.notify("textDocument/didOpen", {
        textDocument: {
          uri: this.getUri(),
          languageId: language,
          text: this.plugin.view.state.doc.toString(),
          version: this.plugin.documentVersion++,
        },
      });
      this.isReady = true;
    })();
  }

  async format() {
    // TODO(virv): if this.isReady
    const res = await this.request("textDocument/formatting", {
      "textDocument": {
        "uri": this.getUri(),
      },
      "options": {
      },
    });
    if (res instanceof Array) {
      const changes = res.map(edit => {
        const start = this.mapPos(edit.range.start);
        const end = this.mapPos(edit.range.end);
        return {
          from: start,
          to: end,
          insert: edit.newText,
        };
      });
      this.plugin.view.dispatch({
        changes,
      });
    }
  }

  stopping() {
    this.isReady = false;
  }

  onNotify(method: string, obj: Object) {
    if (method === "textDocument/publishDiagnostics") {
      if (obj["uri"] === this.getUri()) {
        this.onDiagnostics(obj["diagnostics"]);
      }
    } else {
      console.log("unknown notification", method, obj);
    }
  }

  mapPos(pos: Position): number {
    return mapPosWithDoc(this.plugin.view.state.doc, pos);
  }

  mapOffset(offset: number): Position {
    return mapOffsetWithDoc(this.plugin.view.state.doc, offset);
  }

  onDiagnostics(diagnostics: Object) {
    const diag = (
      diagnostics as Array<{range: Range; message: string; severity: number}>
    )
      .map(({range, message, severity}) => ({
        from: this.mapPos(range.start)!,
        to: this.mapPos(range.end)!,
        severity: (
          {
            1: "error",
            2: "warning",
            3: "info",
            4: "info",
          } as const
        )[severity!],
        message,
      }))
      .filter(
        ({from, to}) =>
          from !== null &&
          to !== null &&
          from !== undefined &&
          to !== undefined,
      )
      .sort((a, b) => {
        switch (true) {
          case a.from < b.from:
            return -1;
          case a.from > b.from:
            return 1;
        }
        return 0;
      });

    this.plugin.view.dispatch(setDiagnostics(this.plugin.view.state, diag));
  }

  message(msg: string) {
    try {
      let msgObj = JSON.parse(msg);
      let id = msgObj["id"];
      if (id === undefined) {
        // TODO
        this.onNotify(msgObj["method"], msgObj["params"]);
        return;
      }
      const [resolve, reject] = this.pending[id];
      if ("error" in msgObj) {
        reject(msgObj["error"]);
      } else {
        resolve(msgObj["result"]);
      }
    } catch (exc) {
      console.log(exc);
    }
  }

  notify(method: string, params: Object) {
    this.sendMessage(
      JSON.stringify({
        jsonrpc: "2.0",
        method,
        params,
      }),
    );
  }

  async request(method: string, params: Object): Promise<Object> {
    let id = this.sendCount;
    this.sendCount += 1;

    let result = new Promise((resolve, reject) => {
      this.pending[id] = [resolve, reject];
    });

    this.sendMessage(
      JSON.stringify({
        jsonrpc: "2.0",
        method,
        params,
        id,
      }),
    );

    return (await result) as Object;
  }
}

const useLast = (values: readonly any[]) => values.reduce((_, v) => v, "");
const languageId = Facet.define<string, string>({combine: useLast});
const lsEventHandler = Facet.define<LSEventHandler, LSEventHandler>({
  combine: useLast,
});

const languageToExtension = {cpp: "cpp", c: "c", python: "py"};

const URI = "file:///solution.";

const completionItemKinds = [
  "text",
  "method",
  "function",
  "constructor",
  "field",
  "variable",
  "class",
  "interface",
  "module",
  "property",
  "unit",
  "value",
  "enum",
  "keyword",
  "snippet",
  "color",
  "file",
  "reference",
  "folder",
  "enummember",
  "constant",
  "struct",
  "event",
  "operator",
  "typeparameter",
];

function toSet(chars: Set<string>) {
  let preamble = "";
  let flat = Array.from(chars).join("");
  const words = /\w/.test(flat);
  if (words) {
    preamble += "\\w";
    flat = flat.replace(/\w/g, "");
  }
  return `[${preamble}${flat.replace(/[^\w\s]/g, "\\$&")}]`;
}

function prefixMatch(options: Completion[]) {
  const first = new Set<string>();
  const rest = new Set<string>();

  for (const {apply} of options) {
    const [initial, ...restStr] = Array.from(apply as string);
    first.add(initial);
    for (const char of restStr) {
      rest.add(char);
    }
  }

  const source = toSet(first) + toSet(rest) + "*$";
  return [new RegExp("^" + source), new RegExp(source)];
}

function formatContents(contents: any): string {
  if (Array.isArray(contents)) {
    return contents.map((c) => formatContents(c) + "\n").join("");
  } else if (typeof contents === "string") {
    return contents;
  } else {
    return contents.value;
  }
}

function documentationToDom(documentation: any): HTMLElement {
  const dom = document.createElement("div");
  dom.classList.add("cm-hover-doc");
  let textContents = formatContents(documentation);
  // Remove <a>
  textContents = textContents.replace(/<a [^>]*>([^<]*)<\/a>/g, "$1");
  textContents = textContents.replace(/</g, "&lt;");
  textContents = textContents.replace(/>/g, "&gt;");
  textContents = textContents.replace(/%([a-z0-9_A-Z]*)/g, "$1");
  textContents = textContents.replace(
    /@c (\S*)/g,
    '<span class="cm-hover-doc-tt">$1</span>',
  );
  textContents = textContents.replace(
    /`([^`]*)`/g,
    '<span class="cm-hover-doc-tt">$1</span>',
  );
  let paramsState = -1;
  let atState = -1;
  let lastWasEmpty = false;
  textContents = textContents
    .split("\n")
    .map((line) => {
      let prefix = "";
      let suffix = "";
      if (!line.startsWith("-") && paramsState === 1) {
        prefix += "</ol>";
        paramsState = 2;
      }
      if (line == "Parameters:" && paramsState === -1) {
        paramsState = 0;
      }
      if (line.startsWith("-") && paramsState === 0) {
        paramsState = 1;
        prefix += "<ol>";
      }
      if (line.startsWith("-") && paramsState === 1) {
        prefix += "<li>";
        suffix += "</li>";
        line = line.substring(1);
      }
      if (line.startsWith("→")) {
        prefix += "<i>returns</i>";
        line = line.substring(1);
      }
      if (!line.startsWith("@") && atState === 0) {
        prefix += "\n";
        atState = 1;
      }
      if (line.startsWith("@") && atState === -1) {
        atState = 0;
        if (!lastWasEmpty) {
          prefix += "\n";
        }
      }
      if (line === "" && atState === 1) {
        suffix += "\n";
        atState = 2;
      }
      lastWasEmpty = line === "";
      let i = 0;
      let nbsp = "";
      for (; i < line.length; i++) {
        if (line[i] !== " ") {
          break;
        }
        nbsp = nbsp + "&nbsp;";
      }
      line =
        '<span class="cm-hover-doc-tt">' + nbsp + "</span>" + line.substring(i);
      line = line + (atState === 1 || paramsState === 1 ? " " : "\n");
      return prefix + line + suffix;
    })
    .join("");
  textContents = textContents.replace(/(@\w+)/gm, "<i>$1</i>");
  textContents = textContents.replace(/\n/g, "<br/>");
  textContents = textContents.replace(/\(\s*([^\)]*?)\s*\)/g, "($1)");
  dom.innerHTML = textContents;
  return dom;
}

class LSPlugin implements PluginValue {
  documentVersion: number = 0;
  lastFullSync: number = 0;

  constructor(public view: EditorView) {
    view.state.facet(lsEventHandler).plugin = this;
  }

  uri(): string {
    const lang = this.view.state.facet(languageId);
    return URI + languageToExtension[lang];
  }

  async update(upd: ViewUpdate) {
    const eventHandler = this.view.state.facet(lsEventHandler);
    const wasSynced = eventHandler.documentIsSynced;
    if (upd.docChanged) {
      eventHandler.documentIsSynced = false;
    }
    if (!eventHandler.isReady) return;
    if (!upd.docChanged && wasSynced) return;
    let contentChanges: {range?: Range; text: string}[] = [
      {
        text: this.view.state.doc.toString(),
      },
    ];
    let fullSync = true;
    const maxFullSyncInterval: number = 256;
    let num_changes = 0;
    upd.changes.iterChanges((_fromA, _toA, _fromB, _toB, _text) => num_changes += 1);
    if (
      wasSynced &&
      eventHandler.capabilities["textDocumentSync"]?.change === 2 &&
      this.documentVersion <= this.lastFullSync + maxFullSyncInterval &&
      // TODO: LS expect ranges to be referring to the partially updated document
      // see https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#didChangeTextDocumentParams
      num_changes == 1
    ) {
      contentChanges = [];
      upd.changes.iterChanges((fromA, toA, _fromB, _toB, text) => {
        contentChanges.push({
          range: {
            start: mapOffsetWithDoc(upd.startState.doc, fromA),
            end: mapOffsetWithDoc(upd.startState.doc, toA),
          },
          text: text.toString(),
        });
      });
      fullSync = true;
    }
    eventHandler.notify("textDocument/didChange", {
      textDocument: {
        uri: this.uri(),
        version: this.documentVersion++,
      },
      contentChanges,
    });
    eventHandler.documentIsSynced = true;
    if (fullSync) {
      this.lastFullSync = this.documentVersion;
    }
  }

  async requestHoverTooltip(
    view: EditorView,
    pos: number,
  ): Promise<Tooltip | null> {
    const eventHandler = view.state.facet(lsEventHandler);
    if (!eventHandler.isReady) return;
    let {line, character} = eventHandler.mapOffset(pos);
    const result = await eventHandler.request("textDocument/hover", {
      textDocument: {uri: this.uri()},
      position: {line, character},
    });
    if (!result) return null;
    const {contents, range} = result as {contents: string; range: any};
    let end: number;
    if (range) {
      pos = eventHandler.mapPos(range.start)!;
      end = eventHandler.mapPos(range.end);
    }
    if (pos === null) return null;
    return {
      pos,
      end,
      create: () => ({dom: documentationToDom(contents)}),
      above: true,
    };
  }

  async requestCompletion(
    context: CompletionContext,
    pos: number,
    {
      triggerKind,
      triggerCharacter,
    }: {
      triggerKind: CompletionTriggerKind;
      triggerCharacter: string | undefined;
    },
  ): Promise<CompletionResult | null> {
    const eventHandler = this.view.state.facet(lsEventHandler);
    if (!eventHandler.isReady) return;
    let {line, character} = eventHandler.mapOffset(pos);

    const result = await eventHandler.request("textDocument/completion", {
      textDocument: {uri: this.uri()},
      position: {line, character},
      context: {
        triggerKind,
        triggerCharacter,
      },
    });

    if (!result) return null;

    const items = "items" in result ? result.items : result;

    let options = (
      items as Array<{
        detail;
        label;
        kind;
        textEdit;
        documentation;
        sortText;
        filterText;
      }>
    ).map(
      ({
        detail,
        label,
        kind,
        textEdit,
        documentation,
        sortText,
        filterText,
      }) => {
        const completion: Completion & {
          filterText: string;
          sortText?: string;
          apply: string;
        } = {
          label,
          detail,
          apply: textEdit?.newText ?? label,
          type: kind && completionItemKinds[kind],
          sortText: sortText ?? label,
          filterText: filterText ?? label,
        };
        if (documentation) {
          completion.info = () => documentationToDom(documentation);
        }
        return completion;
      },
    );

    const [span, match] = prefixMatch(options);
    const token = context.matchBefore(match);

    if (token) {
      pos = token.from;
      const word = token.text.toLowerCase();
      if (/^\w+$/.test(word)) {
        options = options
          .filter(({filterText}) => filterText.toLowerCase().startsWith(word))
          .sort(({apply: a}, {apply: b}) => {
            switch (true) {
              case a.startsWith(token.text) && !b.startsWith(token.text):
                return -1;
              case !a.startsWith(token.text) && b.startsWith(token.text):
                return 1;
            }
            return 0;
          });
      }
    }
    return {
      from: pos,
      options,
    };
  }
}

enum CompletionTriggerKind {
  Invoked = 1,
  TriggerCharacter = 2,
  TriggerForIncompleteCompletions = 3,
}

export class CM6Editor {
  language: Compartment = new Compartment();
  keymap: Compartment = new Compartment();
  dark: Compartment = new Compartment();
  execCallback: () => void = () => {};
  onchangeCallback: () => void = () => {};
  isReadOnly = new Compartment();
  execKeyBinding = Prec.highest(
    keymap.of([
      {
        key: "Mod-Enter",
        run: () => {
          this.execCallback();
          return true;
        },
      },
      {
        key: "Mod-f",
        run: () => {
          this.format();
          return true;
        },
      },
    ]),
  );
  view: EditorView;

  constructor(elementId: string) {
    const element = document.getElementById(elementId);
    if (element === null) {
      throw new Error(`"${elementId}" not found`);
    }
    let plugin: LSPlugin | null = null;
    this.view = new EditorView({
      extensions: [
        lsEventHandler.of(new LSEventHandler((_) => {})),
        keymap.of([indentWithTab]),
        this.keymap.of([]),
        this.execKeyBinding,
        basicSetup,
        this.dark.of(solarizedLight),
        this.language.of(languageId.of("")),
        this.isReadOnly.of(EditorState.readOnly.of(false)),
        ViewPlugin.define((view) => (plugin = new LSPlugin(view))),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            this.onchangeCallback();
          }
        }),
        hoverTooltip(
          (view, pos) => plugin?.requestHoverTooltip(view, pos) ?? null,
        ),
        autocompletion({
          override: [
            async (context) => {
              if (plugin == null) return null;

              const {state, pos, explicit} = context;
              const line = state.doc.lineAt(pos);
              const eventHandler = plugin.view.state.facet(lsEventHandler);
              let trigKind: CompletionTriggerKind =
                CompletionTriggerKind.Invoked;
              let trigChar: string | undefined;
              if (
                !explicit &&
                eventHandler.capabilities?.completionProvider?.triggerCharacters?.includes(
                  line.text[pos - line.from - 1],
                )
              ) {
                trigKind = CompletionTriggerKind.TriggerCharacter;
                trigChar = line.text[pos - line.from - 1];
              }
              if (
                trigKind === CompletionTriggerKind.Invoked &&
                !context.matchBefore(/\w+$/)
              ) {
                return null;
              }
              return await plugin.requestCompletion(context, pos, {
                triggerKind: trigKind,
                triggerCharacter: trigChar,
              });
            },
          ],
        }),
      ],
      parent: element,
    });
  }

  format() {
    let eventHandler = this.view.state.facet(lsEventHandler);
    eventHandler.format();
  }

  setLanguage(lang: string) {
    if (lang === "c") {
      this.view.dispatch({
        effects: this.language.reconfigure([cpp(), languageId.of("c")]),
      });
    } else if (lang === "cpp") {
      this.view.dispatch({
        effects: this.language.reconfigure([cpp(), languageId.of("cpp")]),
      });
    } else if (lang === "python") {
      this.view.dispatch({
        effects: this.language.reconfigure([python(), languageId.of("python")]),
      });
    } else {
      this.view.dispatch({
        effects: this.language.reconfigure(languageId.of("")),
      });
    }
  }

  setDark(dark: boolean) {
    this.view.dispatch({
      effects: this.dark.reconfigure(dark ? solarizedDark : solarizedLight),
    });
  }

  setReadOnly(isReadonly: boolean) {
    this.view.dispatch({
      effects: this.isReadOnly.reconfigure(EditorState.readOnly.of(isReadonly)),
    });
  }

  setKeymap(keymap: string) {
    if (keymap === "vim") {
      this.view.dispatch({effects: this.keymap.reconfigure(vim())});
    } else if (keymap === "emacs") {
      this.view.dispatch({effects: this.keymap.reconfigure(emacs())});
    } else {
      this.view.dispatch({effects: this.keymap.reconfigure([])});
    }
  }

  setExec(exec: () => void) {
    this.execCallback = exec;
  }

  setOnchange(onchange: () => void) {
    this.onchangeCallback = onchange;
  }

  setText(text: string) {
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

  setLanguageServer(sendMessage: (msg: string) => void): LSEventHandler {
    let eventHandler = this.view.state.facet(lsEventHandler);
    eventHandler.sendMessage = sendMessage;
    return eventHandler;
  }
}
