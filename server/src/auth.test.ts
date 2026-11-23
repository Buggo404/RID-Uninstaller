import assert from "node:assert/strict";
import { test } from "node:test";
import { captchaAnswer, captchaPasses, cookieOf, hashPassword, invalidCredentials, newCaptcha, passwordMatches } from "./auth.ts";

test("passwords are stored salted and verify only against themselves", async () => {
  const stored = await hashPassword("correct horse");
  assert.notEqual(stored, await hashPassword("correct horse"));
  assert.ok(await passwordMatches("correct horse", stored));
  assert.ok(!(await passwordMatches("correct horsf", stored)));
  assert.ok(!(await passwordMatches("x", "")));
  assert.ok(!(await passwordMatches("x", "plain$a$b")));
});

test("a captcha can be answered once and expires", () => {
  const first = newCaptcha(1000);
  const answer = captchaAnswer(first.id)!;
  assert.match(answer, /^\d{5}$/);
  assert.ok(!first.svg.includes(answer) && !first.svg.includes("<text"));
  assert.ok(!captchaPasses(first.id, "wrong", 2000));
  assert.ok(!captchaPasses(first.id, answer, 2000), "a wrong answer uses the image up");

  const second = newCaptcha(1000);
  assert.ok(captchaPasses(second.id, captchaAnswer(second.id), 2000));
  const third = newCaptcha(1000);
  assert.ok(!captchaPasses(third.id, captchaAnswer(third.id), 1000 + 10 * 60_000), "expired");
  assert.ok(!captchaPasses(undefined, undefined));
});

test("credentials must be a plain name and a long password that does not repeat it", () => {
  assert.equal(invalidCredentials("root.admin", "a-long-secret"), null);
  for (const [name, password] of [["ab", "a-long-secret"], ["has space", "a-long-secret"], ["admin", "short"], ["admin", "MyAdmin12345"], [5, "a-long-secret"]]) {
    assert.ok(invalidCredentials(name, password), `${name} / ${password}`);
  }
});

test("one cookie is picked out of the header", () => {
  assert.equal(cookieOf("a=1; rid_admin=tok=en; b=2", "rid_admin"), "tok=en");
  assert.equal(cookieOf("a=1", "rid_admin"), undefined);
  assert.equal(cookieOf(undefined, "rid_admin"), undefined);
});
