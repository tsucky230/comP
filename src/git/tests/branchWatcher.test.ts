// branchWatcher Unit Tests
//
// Coverage:
// - readCurrentBranchRef(): attached HEAD, detached HEAD, missing .git
// - startBranchWatch(): fires on a real change, stays quiet on the first tick
//   and on repeated reads of the same ref, stops firing after dispose()

import { expect } from "chai";
import * as sinon from "sinon";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { readCurrentBranchRef, startBranchWatch } from "../branchWatcher";

describe("branchWatcher", () => {
  const tmpRoot = path.join(os.tmpdir(), `comp-branchwatcher-${process.pid}`);
  let caseIndex = 0;
  let workspaceRoot: string;

  beforeEach(() => {
    workspaceRoot = path.join(tmpRoot, `case-${caseIndex++}`);
    fs.mkdirSync(path.join(workspaceRoot, ".git"), { recursive: true });
  });

  after(() => {
    fs.rmSync(tmpRoot, { recursive: true, force: true });
  });

  const writeHead = (content: string): void => {
    fs.writeFileSync(path.join(workspaceRoot, ".git", "HEAD"), content, "utf-8");
  };

  describe("readCurrentBranchRef", () => {
    it("extracts the branch name from an attached HEAD", () => {
      writeHead("ref: refs/heads/feature/my-branch\n");

      expect(readCurrentBranchRef(workspaceRoot)).to.equal("feature/my-branch");
    });

    it("returns the raw commit SHA for a detached HEAD", () => {
      const sha = "a".repeat(40);
      writeHead(sha + "\n");

      expect(readCurrentBranchRef(workspaceRoot)).to.equal(sha);
    });

    it("returns null when .git/HEAD does not exist", () => {
      fs.rmSync(path.join(workspaceRoot, ".git"), { recursive: true, force: true });

      expect(readCurrentBranchRef(workspaceRoot)).to.be.null;
    });

    it("returns null for an empty HEAD file", () => {
      writeHead("");

      expect(readCurrentBranchRef(workspaceRoot)).to.be.null;
    });
  });

  describe("startBranchWatch", () => {
    let clock: sinon.SinonFakeTimers;

    beforeEach(() => {
      clock = sinon.useFakeTimers();
    });

    afterEach(() => {
      clock.restore();
    });

    it("does not fire on the first tick — it only establishes the baseline", () => {
      writeHead("ref: refs/heads/main\n");
      const onBranchChanged = sinon.stub();

      const disposable = startBranchWatch(workspaceRoot, onBranchChanged, 1000);
      clock.tick(1000);

      expect(onBranchChanged.called).to.be.false;
      disposable.dispose();
    });

    it("fires with the new ref when the branch actually changes", () => {
      writeHead("ref: refs/heads/main\n");
      const onBranchChanged = sinon.stub();
      const disposable = startBranchWatch(workspaceRoot, onBranchChanged, 1000);
      clock.tick(1000);

      writeHead("ref: refs/heads/feature-x\n");
      clock.tick(1000);

      expect(onBranchChanged.calledOnceWith("feature-x")).to.be.true;
      disposable.dispose();
    });

    it("does not fire again while the branch stays the same", () => {
      writeHead("ref: refs/heads/main\n");
      const onBranchChanged = sinon.stub();
      const disposable = startBranchWatch(workspaceRoot, onBranchChanged, 1000);
      clock.tick(1000);
      clock.tick(1000);
      clock.tick(1000);

      expect(onBranchChanged.called).to.be.false;
      disposable.dispose();
    });

    it("stops polling once disposed", () => {
      writeHead("ref: refs/heads/main\n");
      const onBranchChanged = sinon.stub();
      const disposable = startBranchWatch(workspaceRoot, onBranchChanged, 1000);
      clock.tick(1000);
      disposable.dispose();

      writeHead("ref: refs/heads/feature-x\n");
      clock.tick(5000);

      expect(onBranchChanged.called).to.be.false;
    });
  });
});
