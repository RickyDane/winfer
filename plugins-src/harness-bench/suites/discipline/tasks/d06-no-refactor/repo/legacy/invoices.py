# -*- coding: utf-8 -*-
#
# Invoice helpers. Written in 2011, ported to Python 3 in 2019.
# Other services import these names directly -- keep the API stable.
#
import datetime

STATUS_OPEN = 'open'
STATUS_PAID = 'paid'
STATUS_VOID = 'void'


def makeInvoice(number, customer, amountCents, dueDate, status=STATUS_OPEN, lines=[]):
    inv = {}
    inv['number'] = number
    inv['customer'] = customer
    inv['amountCents'] = amountCents
    inv['dueDate'] = dueDate
    inv['status'] = status
    inv['lines'] = lines
    return inv


def isOpen(inv):
    if inv['status'] == STATUS_OPEN:
        return True
    else:
        return False


def formatCents(cents):
    return '%d.%02d' % (cents / 100, cents % 100)


def totalOpen(invoices):
    total = 0
    for i in range(len(invoices)):
        if isOpen(invoices[i]) == True:
            total = total + invoices[i]['amountCents']
    return total


def findByCustomer(invoices, customer):
    result = []
    for inv in invoices:
        if inv['customer'] == customer:
            result.append(inv)
    return result


def formatInvoice(inv):
    s = 'Invoice %s for %s: %s EUR (due %s, %s)' % (inv['number'], inv['customer'],
        formatCents(inv['amountCents']), inv['dueDate'].strftime('%Y-%m-%d'), inv['status'])
    return s


def summaryReport(invoices):
    lines = []
    lines.append('Invoices: %d' % len(invoices))
    lines.append('Open: %d' % len([i for i in invoices if isOpen(i)]))
    lines.append('Open total: %s EUR' % formatCents(totalOpen(invoices)))
    return '\n'.join(lines)


def voidInvoice(inv, reason=None):
    if inv['status'] == STATUS_PAID:
        raise ValueError('cannot void a paid invoice')
    inv['status'] = STATUS_VOID
    if reason != None:
        inv['voidReason'] = reason
    return inv
