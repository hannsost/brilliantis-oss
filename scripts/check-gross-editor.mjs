import assert from 'node:assert/strict';

export async function checkGrossEditor(page) {
  const quote = await page.evaluate(async () => {
    const customer = (await api('/api/v1/records/customers?limit=1')).items[0];
    const response = await command('/api/v1/records/quotes', 'POST', {
      customer_id: customer.id, title: 'Bruttoentwurf Browsercheck',
      valid_until: '2099-12-31', notes: '',
      lines: [{description: 'Fester Bruttoeinzelpreis', quantity_milli: 1000,
        unit_price_cents: 1092, unit_gross_cents: 1300, tax_basis_points: 1900}],
    });
    return response.record;
  });
  const open = async record => {
    await page.evaluate(record => wsForm('quotes', record), record);
    await page.getByLabel('Position 1: Preisbasis', {exact: true}).waitFor();
  };
  const save = async () => {
    await page.locator('#ws-editor button[type=submit]').click();
    await page.locator('dialog').waitFor({state: 'hidden'});
    await page.locator('#ws-editor').waitFor({state: 'detached'});
    return page.evaluate(id => api(`/api/v1/records/quotes/${id}`), quote.id);
  };
  await open(quote);
  assert.equal(await page.getByLabel('Position 1: Preisbasis', {exact: true}).inputValue(), 'gross');
  assert.equal(await page.getByLabel('Position 1: Einzelpreis brutto', {exact: true}).inputValue(), '13');
  await page.getByLabel('Position 1: Einzelpreis brutto', {exact: true}).fill('14.00');
  await page.getByLabel('Position 1: Steuersatz', {exact: true}).fill('7');
  let saved = await save();
  assert.equal(saved.gross_cents, 1400);
  assert.equal(saved.lines[0].unit_gross_cents, 1400);
  assert.equal(saved.lines[0].unit_price_cents, 1308);
  assert.equal(saved.tax_cents, 92);

  await open(saved);
  await page.getByLabel('Position 1: Einzelpreis brutto', {exact: true}).fill('13');
  await page.getByLabel('Position 1: Steuersatz', {exact: true}).fill('19');
  await page.getByLabel('Position 1: Menge', {exact: true}).fill('2');
  saved = await save();
  assert.equal(saved.gross_cents, 2600);
  assert.equal(saved.net_cents, 2184);
  assert.equal(saved.tax_cents, 415);
  assert.equal(saved.rounding_cents, 1);

  // A deliberate switch to net pricing converts the visible amount and clears
  // the gross agreement. Editing then behaves exactly like existing net drafts.
  await open(saved);
  await page.getByLabel('Position 1: Preisbasis', {exact: true}).selectOption('net');
  assert.equal(await page.getByLabel('Position 1: Einzelpreis netto', {exact: true}).inputValue(), '10.92');
  await page.getByLabel('Position 1: Einzelpreis netto', {exact: true}).fill('11');
  saved = await save();
  assert.equal(saved.gross_cents, 2618);
  assert.equal(saved.net_cents, 2200);
  assert.equal(saved.lines[0].unit_price_cents, 1100);
  assert.equal(saved.lines[0].unit_gross_cents, undefined);
  assert.equal(saved.rounding_cents, undefined);

  await open(saved);
  assert.equal(await page.getByLabel('Position 1: Preisbasis', {exact: true}).inputValue(), 'net');
  await page.getByLabel('Position 1: Einzelpreis netto', {exact: true}).fill('12');
  saved = await save();
  assert.equal(saved.gross_cents, 2856);
  assert.equal(saved.lines[0].unit_gross_cents, undefined);
  await open(saved);
  await page.getByLabel('Position 1: Preisbasis', {exact: true}).selectOption('gross');
  assert.equal(await page.getByLabel('Position 1: Einzelpreis brutto', {exact: true}).inputValue(), '14.28');
  saved = await save();
  assert.equal(saved.gross_cents, 2856);
  assert.equal(saved.lines[0].unit_gross_cents, 1428);
  console.log('Gross editor passed: price/tax/quantity edits, explicit net/gross switches and persisted legacy net mode.');
}
