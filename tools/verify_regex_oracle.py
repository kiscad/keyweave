#!/usr/bin/env python3
"""Compare shared UTF-8 golden booleans with native C++ std::regex, no input IO."""
import hashlib
import json
import subprocess
import tempfile
from pathlib import Path
root=Path(__file__).resolve().parents[1]
fixture=root/'tests/fixtures/regex/ecmascript-utf8.json'
cases=json.loads(fixture.read_text())['cases']
def literal(text):
    data=text.encode('utf-8')
    return 'std::string("'+''.join(f'\\x{b:02x}' for b in data)+'",'+str(len(data))+')'
lines=['#include <regex>','#include <locale>','#include <string>','#include <iostream>','int main() { std::locale::global(std::locale::classic());']
for i,case in enumerate(cases):
    lines.append(f'{{ std::regex r({literal(case["pattern"])}, std::regex_constants::ECMAScript); bool match=std::regex_search({literal(case["value"])},r); std::cout << {i} << " " << (match ? 1 : 0) << "\\n"; }}')
lines.append('}')
with tempfile.TemporaryDirectory(prefix='keyweave-regex-oracle-') as directory:
    source=Path(directory)/'oracle.cpp';binary=Path(directory)/'oracle'
    source.write_text('\n'.join(lines)+'\n')
    subprocess.run(['c++','-std=c++17','-O0','-Wall','-Wextra','-Werror',str(source),'-o',str(binary)],check=True)
    actual=subprocess.check_output([str(binary)],text=True).splitlines()
expected=[f'{i} {int(case["matches"])}' for i,case in enumerate(cases)]
if actual!=expected:
    failures=[{'case':case,'actual':actual[i] if i<len(actual) else None}for i,case in enumerate(cases)if i>=len(actual)or actual[i]!=expected[i]]
    raise SystemExit(json.dumps({'failures':failures,'actual':actual},ensure_ascii=False,indent=2))
print(json.dumps({'cases_verified':len(cases),'fixture_sha256':hashlib.sha256(fixture.read_bytes()).hexdigest(),'compiler':subprocess.check_output(['c++','--version'],text=True).splitlines()[0],'oracle':'std::regex<char> ECMAScript, classic locale; not macOS runtime'},ensure_ascii=False,indent=2))
