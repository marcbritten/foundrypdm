-- Migration 0009: sheet-metal standards library (MFG reference data).
--
-- A reusable STANDARD the per-item sheet_metal_specs can reference:
-- material x thickness (mm + US gauge) -> density, min bend radius, K-factor,
-- bend deduction and bend allowance (90-degree air bend).
--
-- Two tables:
--   sheet_material_standard_templates — global seed catalog (not org-scoped),
--   sheet_material_standards          — the organization's own library.
-- A fresh organization is seeded idempotently on first access via
-- pdm_db::ensure_sheet_material_standards (copy from the templates, ON CONFLICT
-- DO NOTHING), keeping the PDM's org-isolation invariant (org_id on every WHERE).

CREATE TABLE IF NOT EXISTS sheet_material_standard_templates (
    material_group     TEXT NOT NULL,          -- e.g. 'Aluminum', 'Steel', 'Stainless', 'Brass', 'Copper'
    material           TEXT NOT NULL,          -- e.g. 'Aluminum 5052-H32'
    thickness_mm       DOUBLE PRECISION NOT NULL,
    thickness_gauge    TEXT,                   -- e.g. '16 ga' (US gauge equivalent, where standard)
    density_g_cm3      DOUBLE PRECISION,
    min_bend_radius_mm DOUBLE PRECISION,       -- inside bend radius for a 90-degree air bend
    k_factor           DOUBLE PRECISION,
    bend_deduction_mm  DOUBLE PRECISION,       -- 90-degree bend deduction
    bend_allowance_mm  DOUBLE PRECISION,       -- 90-degree bend allowance
    notes              TEXT NOT NULL DEFAULT '',
    UNIQUE (material, thickness_mm)
);

CREATE TABLE IF NOT EXISTS sheet_material_standards (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id               UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    material_group       TEXT NOT NULL,
    material             TEXT NOT NULL,
    thickness_mm         DOUBLE PRECISION NOT NULL,
    thickness_gauge      TEXT,
    density_g_cm3        DOUBLE PRECISION,
    min_bend_radius_mm   DOUBLE PRECISION,
    k_factor             DOUBLE PRECISION,
    bend_deduction_mm    DOUBLE PRECISION,
    bend_allowance_mm    DOUBLE PRECISION,
    notes                TEXT NOT NULL DEFAULT '',
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, material, thickness_mm)
);
CREATE INDEX IF NOT EXISTS idx_sheet_mat_standards_org ON sheet_material_standards (org_id);

-- Seed catalog: common sheet-metal standards (90-degree air-bend properties).
-- Global reference data, not org-scoped; each organization's copy lives in
-- sheet_material_standards and is created on first access (pdm-db::ensure_sheet_material_standards).
INSERT INTO sheet_material_standard_templates
    (material_group, material, thickness_mm, thickness_gauge, density_g_cm3,
     min_bend_radius_mm, k_factor, bend_deduction_mm, bend_allowance_mm, notes)
VALUES
    ('Aluminum','Aluminum 3003-H14',0.5,NULL,2.7,0.25,0.33,0.85,0.65,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 3003-H14',0.6,'24 ga',2.7,0.3,0.33,1.02,0.78,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 3003-H14',0.9,'20 ga',2.7,0.45,0.33,1.53,1.17,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 3003-H14',1.2,'18 ga',2.7,0.6,0.33,2.04,1.56,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 3003-H14',1.5,'16 ga',2.7,0.75,0.33,2.54,1.96,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 3003-H14',1.9,'14 ga',2.7,0.95,0.33,3.22,2.48,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 3003-H14',2.5,'~12 ga',2.7,1.25,0.33,4.24,3.26,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 3003-H14',3.0,NULL,2.7,1.5,0.33,5.09,3.91,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 5052-H32',0.5,NULL,2.66,0.25,0.33,0.85,0.65,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 5052-H32',0.6,'24 ga',2.66,0.3,0.33,1.02,0.78,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 5052-H32',0.9,'20 ga',2.66,0.45,0.33,1.53,1.17,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 5052-H32',1.2,'18 ga',2.66,0.6,0.33,2.04,1.56,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 5052-H32',1.5,'16 ga',2.66,0.75,0.33,2.54,1.96,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 5052-H32',1.9,'14 ga',2.66,0.95,0.33,3.22,2.48,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 5052-H32',2.5,'~12 ga',2.66,1.25,0.33,4.24,3.26,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 5052-H32',3.0,NULL,2.66,1.5,0.33,5.09,3.91,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 6061-T6',1.0,NULL,2.7,1.0,0.4,1.8,2.2,'hard temper - allow the larger radius; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 6061-T6',1.5,'16 ga',2.7,1.5,0.4,2.7,3.3,'hard temper - allow the larger radius; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 6061-T6',1.9,'14 ga',2.7,1.9,0.4,3.42,4.18,'hard temper - allow the larger radius; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 6061-T6',2.5,'~12 ga',2.7,2.5,0.4,4.5,5.5,'hard temper - allow the larger radius; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 6061-T6',3.0,NULL,2.7,3.0,0.4,5.4,6.6,'hard temper - allow the larger radius; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 1100',0.5,NULL,2.7,0.25,0.33,0.85,0.65,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 1100',0.9,'20 ga',2.7,0.45,0.33,1.53,1.17,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 1100',1.5,'16 ga',2.7,0.75,0.33,2.54,1.96,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Aluminum','Aluminum 1100',2.5,'~12 ga',2.7,1.25,0.33,4.24,3.26,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Mild steel A36',0.6,'24 ga',7.85,0.6,0.4,1.08,1.32,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Mild steel A36',0.9,'20 ga',7.85,0.9,0.4,1.62,1.98,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Mild steel A36',1.2,'18 ga',7.85,1.2,0.4,2.16,2.64,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Mild steel A36',1.5,'16 ga',7.85,1.5,0.4,2.7,3.3,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Mild steel A36',1.9,'14 ga',7.85,1.9,0.4,3.42,4.18,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Mild steel A36',2.5,'~12 ga',7.85,2.5,0.4,4.5,5.5,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Mild steel A36',3.2,'~10 ga',7.85,3.2,0.4,5.76,7.04,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Mild steel A36',4.0,'8 ga',7.85,4.0,0.4,7.2,8.8,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Cold-rolled steel 1008',0.6,'24 ga',7.85,0.6,0.4,1.08,1.32,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Cold-rolled steel 1008',0.9,'20 ga',7.85,0.9,0.4,1.62,1.98,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Cold-rolled steel 1008',1.2,'18 ga',7.85,1.2,0.4,2.16,2.64,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Cold-rolled steel 1008',1.5,'16 ga',7.85,1.5,0.4,2.7,3.3,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Cold-rolled steel 1008',1.9,'14 ga',7.85,1.9,0.4,3.42,4.18,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Cold-rolled steel 1008',2.5,'~12 ga',7.85,2.5,0.4,4.5,5.5,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Cold-rolled steel 1008',3.2,'~10 ga',7.85,3.2,0.4,5.76,7.04,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Cold-rolled steel 1008',4.0,'8 ga',7.85,4.0,0.4,7.2,8.8,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Galvanized steel G90',0.6,'24 ga',7.85,0.9,0.42,1.19,1.81,'bend across the grain; the zinc coat cracks below the min radius; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Galvanized steel G90',0.9,'20 ga',7.85,1.35,0.42,1.79,2.71,'bend across the grain; the zinc coat cracks below the min radius; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Galvanized steel G90',1.2,'18 ga',7.85,1.8,0.42,2.38,3.62,'bend across the grain; the zinc coat cracks below the min radius; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Galvanized steel G90',1.5,'16 ga',7.85,2.25,0.42,2.98,4.52,'bend across the grain; the zinc coat cracks below the min radius; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Galvanized steel G90',1.9,'14 ga',7.85,2.85,0.42,3.77,5.73,'bend across the grain; the zinc coat cracks below the min radius; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Steel','Galvanized steel G90',2.5,'~12 ga',7.85,3.75,0.42,4.96,7.54,'bend across the grain; the zinc coat cracks below the min radius; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Stainless','Stainless steel 304',0.5,NULL,8.0,0.5,0.5,0.82,1.18,'high springback - verify on the first part; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Stainless','Stainless steel 304',0.6,'24 ga',8.0,0.6,0.5,0.99,1.41,'high springback - verify on the first part; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Stainless','Stainless steel 304',0.9,'20 ga',8.0,0.9,0.5,1.48,2.12,'high springback - verify on the first part; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Stainless','Stainless steel 304',1.0,NULL,8.0,1.0,0.5,1.64,2.36,'high springback - verify on the first part; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Stainless','Stainless steel 304',1.2,'18 ga',8.0,1.2,0.5,1.97,2.83,'high springback - verify on the first part; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Stainless','Stainless steel 304',1.5,'16 ga',8.0,1.5,0.5,2.47,3.53,'high springback - verify on the first part; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Stainless','Stainless steel 304',1.9,'14 ga',8.0,1.9,0.5,3.12,4.48,'high springback - verify on the first part; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Stainless','Stainless steel 304',2.5,'~12 ga',8.0,2.5,0.5,4.11,5.89,'high springback - verify on the first part; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Stainless','Stainless steel 316',0.9,'20 ga',8.0,0.9,0.5,1.48,2.12,'high springback - verify on the first part; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Stainless','Stainless steel 316',1.2,'18 ga',8.0,1.2,0.5,1.97,2.83,'high springback - verify on the first part; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Stainless','Stainless steel 316',1.5,'16 ga',8.0,1.5,0.5,2.47,3.53,'high springback - verify on the first part; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Stainless','Stainless steel 316',1.9,'14 ga',8.0,1.9,0.5,3.12,4.48,'high springback - verify on the first part; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Stainless','Stainless steel 316',2.5,'~12 ga',8.0,2.5,0.5,4.11,5.89,'high springback - verify on the first part; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Brass','Brass C360',0.6,'24 ga',8.5,0.3,0.4,0.95,0.85,'work-hardens - anneal if bend lines are tight; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Brass','Brass C360',0.9,'20 ga',8.5,0.45,0.4,1.43,1.27,'work-hardens - anneal if bend lines are tight; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Brass','Brass C360',1.2,'18 ga',8.5,0.6,0.4,1.9,1.7,'work-hardens - anneal if bend lines are tight; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Brass','Brass C360',1.5,'16 ga',8.5,0.75,0.4,2.38,2.12,'work-hardens - anneal if bend lines are tight; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Brass','Brass C360',1.9,'14 ga',8.5,0.95,0.4,3.01,2.69,'work-hardens - anneal if bend lines are tight; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Brass','Brass C377',0.6,'24 ga',8.5,0.3,0.4,0.95,0.85,'work-hardens - anneal if bend lines are tight; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Brass','Brass C377',0.9,'20 ga',8.5,0.45,0.4,1.43,1.27,'work-hardens - anneal if bend lines are tight; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Brass','Brass C377',1.2,'18 ga',8.5,0.6,0.4,1.9,1.7,'work-hardens - anneal if bend lines are tight; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Brass','Brass C377',1.5,'16 ga',8.5,0.75,0.4,2.38,2.12,'work-hardens - anneal if bend lines are tight; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Brass','Brass C377',1.9,'14 ga',8.5,0.95,0.4,3.01,2.69,'work-hardens - anneal if bend lines are tight; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Copper','Copper C110 (soft)',0.6,'24 ga',8.96,0.3,0.4,0.95,0.85,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Copper','Copper C110 (soft)',0.9,'20 ga',8.96,0.45,0.4,1.43,1.27,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Copper','Copper C110 (soft)',1.2,'18 ga',8.96,0.6,0.4,1.9,1.7,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Copper','Copper C110 (soft)',1.5,'16 ga',8.96,0.75,0.4,2.38,2.12,'typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Copper','Copper C10200 (hard)',0.9,'20 ga',8.96,0.9,0.4,1.62,1.98,'hard temper - allow the larger radius; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Copper','Copper C10200 (hard)',1.2,'18 ga',8.96,1.2,0.4,2.16,2.64,'hard temper - allow the larger radius; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Copper','Copper C10200 (hard)',1.5,'16 ga',8.96,1.5,0.4,2.7,3.3,'hard temper - allow the larger radius; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA'),
    ('Copper','Copper C10200 (hard)',1.9,'14 ga',8.96,1.9,0.4,3.42,4.18,'hard temper - allow the larger radius; typical 90 deg air bend, V-die opening ~8t; BA = 1.5708*(IR + K*t); BD = 2*(IR+t) - BA');
