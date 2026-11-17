# Authored stdio fixture. It only writes protocol.jsonl inside the temporary repo.
import json, sys
source, uri = '', ''
log = open('protocol.jsonl', 'a')
while True:
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if line == b'\r\n': break
        key, value = line.decode().split(':', 1)
        headers[key.lower()] = value.strip()
    message = json.loads(sys.stdin.buffer.read(int(headers['content-length'])))
    log.write(json.dumps(message, ensure_ascii=False) + '\n'); log.flush()
    method, params = message.get('method'), message.get('params', {})
    if method == 'textDocument/didOpen':
        source, uri = params['textDocument']['text'], params['textDocument']['uri']
    elif method == 'textDocument/didChange': source = params['contentChanges'][-1]['text']
    if 'id' not in message: continue
    result = None
    pos = lambda line, char: {'line':line, 'character':char}
    span = lambda line, a, b: {'start':pos(line,a), 'end':pos(line,b)}
    if method == 'initialize':
        result = {'capabilities': {'textDocumentSync':1, 'positionEncoding':'utf-16', 'definitionProvider':True, 'referencesProvider':True, 'completionProvider':{'resolveProvider':True}, 'documentFormattingProvider':True, 'diagnosticProvider':{'interFileDependencies':False, 'workspaceDiagnostics':False}, 'documentSymbolProvider':True, 'hoverProvider':True, 'signatureHelpProvider':{'triggerCharacters':['(', ',']}}}
    elif method in ['textDocument/definition', 'textDocument/references']:
        result = [{'uri':uri, 'range':span(1,6,11)}]
    elif method == 'textDocument/completion':
        result = {'isIncomplete':False,'items':[{'label':'dirtyMethod','detail':'(count: number): string','insertTextFormat':2,'textEdit':{'range':span(2,0,5),'newText':'dirtyMethod(${1:count})$0'},'additionalTextEdits':[{'range':span(1,0,0),'newText':'// import\n'}], 'data':{'fixture':True}}]}
        result['items'].extend([
            {'label':'mirrored snippet','insertTextFormat':2,'textEdit':{'range':span(2,0,5),'newText':'<${1|one,two|}>${2:${3:inside}}</$1>'}},
            {'label':'unsupported transform','insertTextFormat':2,'textEdit':{'range':span(2,0,5),'newText':'${1/foo/bar/}'}}
        ])
    elif method == 'completionItem/resolve':
        result = dict(params); result['documentation'] = {'kind':'markdown','value':'Fixture **documentation**'}
    elif method == 'textDocument/diagnostic':
        result = {'kind':'full','items':[{'range':span(1,6,11),'severity':2,'source':'fixture','code':'W1','message':'dirty draft warning'}]}
    elif method == 'textDocument/documentSymbol':
        result = [{'name':'Container','kind':5,'range':{'start':pos(1,0),'end':pos(2,5)},'selectionRange':span(1,6,11),'children':[{'name':'dirty','kind':13,'range':span(2,0,5),'selectionRange':span(2,0,5)}]}]
    elif method == 'textDocument/formatting':
        result = [{'range':{'start':pos(0,0),'end':pos(2,5)}, 'newText':source.replace('dirty','formatted')}]
    elif method == 'textDocument/hover': result = {'contents':{'kind':'plaintext','value':'const dirty: number'}}
    elif method == 'textDocument/signatureHelp':
        result = {'signatures':[{'label':'f(value: string, count: number)','documentation':'Fixture signature','parameters':[{'label':'value: string'},{'label':'count: number','documentation':'Active count'}]}],'activeSignature':0,'activeParameter':1}
    body = json.dumps({'jsonrpc':'2.0','id':message['id'],'result':result},ensure_ascii=False).encode()
    sys.stdout.buffer.write(('Content-Length: %s\r\n\r\n' % len(body)).encode()+body);sys.stdout.buffer.flush()
