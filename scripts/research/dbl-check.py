"""Numerical check of Abdelhakmi & Lim, "Dynamic Black-Litterman"
(arXiv:2404.18822v2; Operations Research 2025). Notes in docs/39.

Part A checks the paper's closed forms against brute-force Gaussian
conditioning and finite differences, on the paper's own 5-asset example (§7).
Part B re-runs the §7.1 Monte Carlo: dynamic BL (DBL, eq. 39) against
single-period BL re-solved at each rebalance (RCBL, eq. 40).

Run: uv run --with numpy python scripts/research/dbl-check.py
"""
import numpy as np

inv = np.linalg.inv

# §7 parameters, copied from the paper (p. 30-31).
MU = np.array([0.0320, 0.0447, 0.0269, 0.0679, 0.0672])
SIG = np.array([[0.0641, 0.0175, 0.0086, 0.0266, 0.0363],
                [0.0175, 0.1191, 0.0234, 0.0303, 0.0353],
                [0.0086, 0.0234, 0.1154, 0.0322, 0.0278],
                [0.0266, 0.0303, 0.0322, 0.1230, 0.0431],
                [0.0363, 0.0353, 0.0278, 0.0431, 0.1679]])
P = np.array([[1, -1, 0, 0, 0],    # A - B
              [1, 0, 0, 0, -1],    # A - F
              [0, 0, 1, 0, 0]], float)  # C ; no view on D
RF, T, N, K = 0.03, 1.0, 5, 3
NAMES = "ABCDF"
MUX = MU - np.diag(SIG) / 2
SI = inv(SIG)
L = np.linalg.cholesky(SIG)
PSP = P @ SIG @ P.T


def omega(alpha):                      # eq. 63
    return alpha * PSP


def beta1(Om):                         # Prop 1
    return SIG @ P.T @ inv(PSP + Om) / T


def beta2(Om, t):                      # Prop 1
    return SIG @ P.T @ inv((T - t) * PSP + T * Om) @ P


def eta(Om, t):                        # eq. 33
    return -P.T @ inv((T - t) * PSP + T * Om) @ P


def M(Om, g, t):                       # Theorem 2
    Q = (1 - t / T) * P.T @ inv(Om) @ P
    return (g - 1) * Q @ inv(g * SI + Q)


def alpha_t(Om, y, t):                 # eq. 33
    ex = t * (MUX + beta1(Om) @ (y - T * P @ MUX))
    return MU + beta1(Om) @ (y - T * P @ MUX) - SIG @ eta(Om, t) @ ex


def b(Om, g, y, t):                    # eq. 37
    return M(Om, g, t) @ SI @ (alpha_t(Om, y, t) - RF)


def close(a, b_, tol, what):
    err = np.max(np.abs(np.asarray(a) - np.asarray(b_)))
    assert err < tol, f"{what}: max error {err:.2e} >= {tol:.0e}"
    print(f"  ok  {what:<62} max err {err:.1e}")


def part_a():
    print("PART A - closed forms vs brute force (paper's 5-asset example)\n")
    rng = np.random.default_rng(0)
    Om, g = omega(0.4), 3.0

    # A1. Prop 1 drift vs direct conditioning of the joint Gaussian
    #     (X(t+h), X(t), Y(0,T)), finite h.
    for t in (0.1, 0.5, 0.9):
        h = 1e-5
        x = rng.normal(t * MUX, np.sqrt(t * np.diag(SIG)))
        y = rng.normal(T * P @ MUX, 0.3)
        C22 = np.block([[t * SIG, t * SIG @ P.T],
                        [t * P @ SIG, T * (PSP + Om)]])
        C12 = np.hstack([t * SIG, (t + h) * SIG @ P.T])
        z = np.concatenate([x - t * MUX, y - T * P @ MUX])
        brute = ((t + h) * MUX + C12 @ inv(C22) @ z - x) / h
        ex = t * (MUX + beta1(Om) @ (y - T * P @ MUX))
        formula = MUX + beta1(Om) @ (y - T * P @ MUX) + beta2(Om, t) @ (ex - x)
        close(brute, formula, 1e-4, f"A1 Prop 1 conditional drift, t={t}")

    # A2. Prop 4 covariance: tS - t^2/T SP'(PSP'+O)^-1 PS == L(tI - t^2 H)L'
    H = (P @ L).T @ inv(PSP + Om) @ (P @ L) / T
    for t in (0.3, 0.8):
        direct = t * SIG - t * t / T * SIG @ P.T @ inv(PSP + Om) @ P @ SIG
        close(direct, L @ (t * np.eye(N) - t * t * H) @ L.T, 1e-12,
              f"A2 Prop 4 bridge covariance, t={t}")

    # A3. Hitting times (eq. 21): all > T (Prop 5) and increasing in Omega.
    assert all(np.abs(P @ L[:, j]).sum() > 0 for j in range(N)), "PL_j = 0"
    print("\n  hitting times T~_i (years; T = 1). Larger = view pulls weaker:")
    prev = None
    for a in (0.1, 0.4, 0.8, 2.0):
        Hh = (P @ L).T @ inv(PSP + omega(a)) @ (P @ L) / T
        Tt = 1 / np.diag(Hh)
        assert np.all(Tt > T), "Prop 5: T~ > T"
        if prev is not None:
            assert np.all(Tt > prev), "Prop 5: T~ increasing in Omega"
        prev = Tt
        print(f"    alpha={a:<4} " + "  ".join(f"{n}:{v:7.2f}" for n, v in zip(NAMES, Tt)))
    print("  ok  A3 Prop 5 (T~ > T, increasing in Omega) for alpha 0.1..2.0\n")

    # Example 1 (one asset): T~ = T(1 + w^2/s^2).
    s2, w2 = 0.04, 0.01
    one = T * ((np.sqrt(s2)) ** 2 / (s2 + w2)) ** -1
    close(one, T * (1 + w2 / s2), 1e-12, "A3 Example 1 one-asset hitting time")

    # A4. Riccati (eq. 30): closed form A = M eta solves it, is symmetric,
    #     negative definite for t < T, and equals the alternative form.
    def A(t):
        return M(Om, g, t) @ eta(Om, t)

    for t in (0.1, 0.5, 0.9):
        e, a_ = eta(Om, t), A(t)
        d = 1e-6
        dA = (A(t + d) - A(t - d)) / (2 * d)
        res = dA + (1 - g) / g * e @ SIG @ e + (a_ @ SIG @ e + e @ SIG @ a_) / g \
            + a_ @ SIG @ a_ / g
        close(res, 0, 1e-6, f"A4 Riccati eq.30 residual of closed form, t={t}")
        close(a_, a_.T, 1e-12, f"A4 A(t) symmetric, t={t}")
        alt = -e @ inv(g / ((g - 1) * (T - t)) * SI + e) @ e
        close(a_, alt, 1e-10, f"A4 A(t) = alternative form (p.21), t={t}")
        ev = np.linalg.eigvalsh((a_ + a_.T) / 2)
        assert ev.max() <= 1e-12, "A(t) should be negative semi-definite"
    print("  note A(t) eigenvalues at t=0.5:",
          np.round(np.linalg.eigvalsh(A(0.5)), 4),
          "(rank K=3: zero in directions no view touches)")

    # A5. Linear ODE (eq. 31): closed-form b(t) solves it. Also test the
    #     appendix's stated derivative d(alpha_t)/dt (p. ec23).
    y = T * P @ MUX + np.array([0.15, -0.10, 0.05])
    for t in (0.2, 0.6):
        e, a_, al = eta(Om, t), A(t), alpha_t(Om, y, t)
        d = 1e-6
        db = (b(Om, g, y, t + d) - b(Om, g, y, t - d)) / (2 * d)
        bb = b(Om, g, y, t)
        res = db + (e + a_) @ SIG @ bb / g + (1 - g) / g * (e + a_) @ (al - RF) \
            + a_ @ (al - np.diag(SIG) / 2)
        close(res, 0, 1e-6, f"A5 eq.31 residual of closed-form b(t), t={t}")
        dal = (alpha_t(Om, y, t + d) - alpha_t(Om, y, t - d)) / (2 * d)
        with_sigma = -SIG @ e @ (al - np.diag(SIG) / 2)
        as_printed = -e @ (al - np.diag(SIG) / 2)
        close(dal, with_sigma, 1e-6, f"A5 d(alpha)/dt = -Sigma eta (alpha - diag/2), t={t}")
        gap = np.max(np.abs(dal - as_printed))
        print(f"  note p.ec23 prints d(alpha)/dt = -eta(alpha - diag/2): off by {gap:.2e}"
              " (missing Sigma; typo only, next line is right)")

    # A6. Corollary 1: (1/g)(I+M)Sigma^-1 == (1/g) Sigma_DBL^-1.
    for t in (0.0, 0.5):
        Bt = inv(SI + (1 - t / T) * P.T @ inv(Om) @ P)
        sdbl = Bt + (SIG - Bt) / g
        close(inv((np.eye(N) + M(Om, g, t)) @ SI), sdbl, 1e-12,
              f"A6 Corollary 1 Sigma_DBL identity, t={t}")

    # A7. Hedge grows with view precision, vanishes at T and at log utility.
    for a in (0.4, 0.8, 2.0):
        ev = np.sort(np.linalg.eigvals(M(omega(a), g, 0.0)).real)[::-1]
        print(f"  M(0) eigenvalues, alpha={a}, gamma=3: {np.round(ev, 3)}")
    assert np.trace(M(omega(0.4), g, 0)) > np.trace(M(omega(0.8), g, 0))
    close(M(Om, g, T), 0, 1e-12, "A7 M(T) = 0 (hedge vanishes at horizon)")
    close(M(Om, 1.0, 0.3), 0, 1e-12, "A7 M = 0 when gamma = 1 (log utility)")

    # A8. Example 3: an old view's weight decays linearly with its age.
    for t in (0.25, 0.75):
        direct = (T - t) * SIG - (T - t) ** 2 * SIG @ P.T @ inv((T - t) * PSP + T * Om) @ P @ SIG
        close(direct, (T - t) * inv(SI + (1 - t / T) * P.T @ inv(Om) @ P), 1e-12,
              f"A8 Example 3 aged-view covariance, t={t}")
    print()


def simulate(alpha, gamma, every, paths, steps=252, seed=7, kinds=("DBL", "RCBL", "none")):
    """§7.1: prices from eq. 8, views from eq. 10; hold shares between rebalances."""
    Om, dt = omega(alpha), T / steps
    LO = np.linalg.cholesky(Om)

    def increments(rng):
        for _ in range(steps):
            yield MUX * dt + rng.standard_normal((paths, N)) @ L.T * np.sqrt(dt)

    XT = sum(increments(np.random.default_rng(seed)))
    Y = XT @ P.T + np.sqrt(T) * np.random.default_rng(seed + 1).standard_normal((paths, K)) @ LO.T
    tilt = (Y - T * P @ MUX) @ beta1(Om).T

    def policy(kind, t, X):
        mt = MU + tilt + (t * (MUX + tilt) - X) @ beta2(Om, t).T
        if kind == "DBL":
            return (mt - RF) @ ((np.eye(N) + M(Om, gamma, t)) @ SI).T / gamma
        if kind in ("RCBL", "RCBLa"):
            prec = SI + (1 - t / T) * P.T @ inv(Om) @ P
            # eq. 40 uses the log-return mean; RCBLa uses the arithmetic mean
            # to separate that handicap from the missing hedge.
            mean = mt - np.diag(SIG) / 2 if kind == "RCBL" else mt
            return (mean - RF) @ prec.T / gamma
        return np.tile(SI @ (MU - RF) / gamma, (paths, 1))     # no views (Merton)

    out = {"lev": np.abs(policy("DBL", 0.0, np.zeros((paths, N)))).sum(1).mean()}
    for kind in kinds:
        X = np.zeros((paths, N))
        h = np.zeros((paths, N))
        cash = np.ones(paths)
        turnover = np.zeros(paths)
        for k, dx in enumerate(increments(np.random.default_rng(seed))):
            if k % every == 0:
                Z = cash + h.sum(1)
                new = policy(kind, k * dt, X) * Z[:, None]
                turnover += np.abs(new - h).sum(1) / np.maximum(np.abs(Z), 1e-12)
                h, cash = new, Z - new.sum(1)
            X += dx
            h *= np.exp(dx)
            cash *= np.exp(RF * dt)
        ZT = cash + h.sum(1)
        ruined = int((ZT <= 0).sum())
        U = np.maximum(ZT, 1e-4) ** (1 - gamma) / (1 - gamma)
        cer = np.log(((1 - gamma) * U.mean()) ** (1 / (1 - gamma))) / T
        out[kind] = (cer, turnover.mean(), ruined)
    return out


def part_b(paths):
    print(f"PART B - §7.1 Monte Carlo, {paths} paths, T=1y, 252 steps, same paths for all")
    print("  CER = certainty-equivalent annual log return. Turnover = traded value / wealth, summed over the year.")
    print("  'none' = Merton portfolio ignoring views. ruined = paths with terminal wealth <= 0 (floored at 1e-4).\n")
    print(f"  {'alpha':>5} {'gamma':>5} {'rebal':>7} | {'CER DBL':>8} {'CER RCBL':>8} {'CER none':>8} |"
          f" {'turn DBL':>8} {'turn RCBL':>9} | ruined D/R")
    for alpha in (0.4, 0.8):
        for gamma in (2.0, 3.0, 5.0):
            for every, name in ((1, "daily"), (5, "weekly"), (21, "monthly")):
                r = simulate(alpha, gamma, every, paths)
                print(f"  {alpha:>5} {gamma:>5} {name:>7} | {r['DBL'][0]:8.3f} {r['RCBL'][0]:8.3f}"
                      f" {r['none'][0]:8.3f} | {r['DBL'][1]:8.1f} {r['RCBL'][1]:9.1f} |"
                      f" {r['DBL'][2]}/{r['RCBL'][2]}")
            print(f"        DBL mean gross exposure at t=0: {r['lev']:.1f}x wealth\n")


def part_c(paths):
    # Omega = alpha*P Sigma P' makes corr(view, realised P X(T)) = 1/sqrt(1+alpha).
    # The paper's alpha = 0.4 / 0.8 means correlations 0.85 / 0.75.
    print("PART C - same experiment with weaker views, gamma = 3")
    print("  RCBLa = eq. 40 with the arithmetic drift instead of the log drift.")
    print(f"  {'alpha':>6} {'corr(view,outcome)':>18} {'M(0) eig':>8} {'rebal':>7} |"
          f" {'CER DBL':>8} {'CER RCBL':>8} {'CER RCBLa':>9} {'CER none':>8} | ruined D/R/Ra")
    for alpha in (0.4, 0.8, 9.0, 99.0):
        m = np.linalg.eigvals(M(omega(alpha), 3.0, 0.0)).real.max()
        for every, name in ((1, "daily"), (5, "weekly")):
            r = simulate(alpha, 3.0, every, paths, kinds=("DBL", "RCBL", "RCBLa", "none"))
            print(f"  {alpha:>6} {1 / np.sqrt(1 + alpha):>18.3f} {m:>8.3f} {name:>7} |"
                  f" {r['DBL'][0]:8.3f} {r['RCBL'][0]:8.3f} {r['RCBLa'][0]:9.3f} {r['none'][0]:8.3f} |"
                  f" {r['DBL'][2]}/{r['RCBL'][2]}/{r['RCBLa'][2]}")


def part_d():
    # Toy, not calibrated: one price, sigma 40%/y, lock for half a year at
    # today's price K = S0 = 100. The holder is paid (S_T - K)+, a call.
    # A view "price +30% by then" with noise variance T*w2 updates log S_T by
    # the same Bayes rule as eq. 4 (Example 1 at t = 0).
    from math import erf, exp, log, sqrt

    def ncdf(z):
        return 0.5 * (1 + erf(z / sqrt(2)))

    def call(m, v, s0=100.0, k=100.0):
        d2 = (m + log(s0 / k)) / sqrt(v)
        return s0 * exp(m + v / 2) * ncdf(d2 + sqrt(v)) - k * ncdf(d2)

    s2, t, view = 0.16, 0.5, log(1.30)
    m0, v0 = -s2 / 2 * t, s2 * t
    print("\nPART D - toy price lock, S0 = K = 100, sigma 40%/y, 6 months, zero rates")
    print(f"  no view                      fair premium {call(m0, v0):6.2f}")
    for w2, label in ((s2, "view +30%, noise = market"), (9 * s2, "view +30%, noise = 9x market")):
        w = v0 / (v0 + w2 * t)
        print(f"  {label:<28} fair premium {call(m0 + w * (view - m0), v0 * (1 - w)):6.2f}"
              f"   (corr(view, outcome) = {sqrt(w):.2f})")


if __name__ == "__main__":
    part_a()
    part_b(paths=40000)
    part_c(paths=40000)
    part_d()
