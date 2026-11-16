#!/usr/bin/env python3
"""Repair only the current, authored UI QA fixture. Never commit or change sources."""
from pathlib import Path
import subprocess


def git(root, *args):
    return subprocess.run(["/usr/bin/git", "-C", str(root), *args],
                          check=True, capture_output=True, text=True).stdout.strip()


root = Path(Path('/tmp/pokarak-ui-fixture-path').read_text().strip()).resolve(strict=True)
if not root.parent.name.startswith('pokarak-ui-check-') or root.name != 'sample-fullstack':
    raise SystemExit('Refusing a path outside the authored UI QA sample')
roots = (root, root.parent / 'api-worktree')
for target in roots:
    if '검증용' not in (target / 'README.md').read_text():
        raise SystemExit(f'Refusing non-QA README: {target}')
    if 'origin' in git(target, 'remote').splitlines():
        raise SystemExit(f'Refusing fixture with origin: {target}')
    if Path(git(target, 'rev-parse', '--show-toplevel')).resolve() != target:
        raise SystemExit(f'Not a worktree root: {target}')
    if not (target / 'backend/src/main/java').is_dir() or not (target / 'backend/src/main/kotlin').is_dir():
        raise SystemExit(f'Missing expected QA source layout: {target}')

pom = '''<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <groupId>dev.pokarak.fixture</groupId>
  <artifactId>ui-analysis-qa</artifactId>
  <version>1.0</version>
  <properties>
    <maven.compiler.release>17</maven.compiler.release>
    <kotlin.version>2.2.21</kotlin.version>
    <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>
  </properties>
  <dependencies>
    <dependency>
      <groupId>org.jetbrains.kotlin</groupId>
      <artifactId>kotlin-stdlib</artifactId>
      <version>${kotlin.version}</version>
    </dependency>
  </dependencies>
  <build>
    <sourceDirectory>backend/src/main/java</sourceDirectory>
    <plugins>
      <plugin>
        <groupId>org.jetbrains.kotlin</groupId>
        <artifactId>kotlin-maven-plugin</artifactId>
        <version>${kotlin.version}</version>
        <executions>
          <execution>
            <id>kotlin-compile</id><phase>compile</phase>
            <goals><goal>compile</goal></goals>
          </execution>
        </executions>
        <configuration>
          <jvmTarget>17</jvmTarget>
          <sourceDirs>
            <source>backend/src/main/kotlin</source>
            <source>backend/src/main/java</source>
          </sourceDirs>
        </configuration>
      </plugin>
      <plugin>
        <groupId>org.apache.maven.plugins</groupId>
        <artifactId>maven-compiler-plugin</artifactId>
        <version>3.14.1</version>
        <configuration><release>17</release></configuration>
      </plugin>
    </plugins>
  </build>
</project>
'''
for target in roots:
    existing = [p for name in ('pom.xml', 'build.gradle', 'build.gradle.kts', 'settings.gradle', 'settings.gradle.kts')
                for p in target.rglob(name)]
    if existing:
        print(f'Preserved existing build configuration: {target}')
        continue
    # Exclusive creation also prevents overwriting a file added after inspection.
    with (target / 'pom.xml').open('x') as file:
        file.write(pom)
    print(f'Added QA Maven configuration: {target / "pom.xml"}')
